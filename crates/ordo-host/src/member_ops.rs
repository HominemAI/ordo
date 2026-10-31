//! The membership commands (`plan-enrolment.md` E3, checklist 22h):
//! `std.space.admit`, `std.space.setRole`, `std.space.suspend`,
//! `std.space.reinstate` and `std.space.remove`.
//!
//! Until these existed the only writer of `members` in production was the
//! founding owner that provisioning writes (checklist 21b). A tenant could
//! never have a second member.
//!
//! # Who may
//!
//! **The space's owner, and nobody else.** A manager administers who may do
//! what inside a space through the ops the program declares, and a manager
//! who could admit an owner could make themselves unremovable. The
//! no-escalation rule that would let a manager admit up to their own level
//! is proposed in `security-hardening-review-2.md` and not settled, so this
//! does not build it. The list is [`crate::platform_ops::owner_only`].
//!
//! # The last owner
//!
//! A space with no active owner is the one state nothing can repair from
//! inside. So the last active owner cannot be removed, suspended or given a
//! lower role. The rule is inside the statement that writes, with the owner
//! rows locked, so two owners removing each other at the same moment cannot
//! both succeed.
//!
//! # Refused on the next request
//!
//! Each command ends by rebuilding the door on this replica, through the
//! hook the runtime hands in, and announcing on `ordo.members` so every
//! other replica does the same. A suspension is therefore refused on the
//! next request here and within a notification everywhere else, and the
//! refresh timer is only the ceiling. When the hook fails the answer says so
//! rather than claiming an effect it did not confirm.

use std::sync::Arc;

use futures::future::BoxFuture;
use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_exec::route::PoolResolver;
use ordo_exec::{sql_lit, Pool};
use ordo_identity::directory::{random_principal_suffix, DirectoryStore};
use ordo_identity::principal::PrincipalId;
use ordo_secevent::{EventKind, Observation, Recorder, Relationship, Subject};
use serde_json::json;

pub const ADMIT: &str = "std.space.admit";
pub const SET_ROLE: &str = "std.space.setRole";
pub const SUSPEND: &str = "std.space.suspend";
pub const REINSTATE: &str = "std.space.reinstate";
pub const REMOVE: &str = "std.space.remove";

/// Every command of this module, for registration and for the owner-only
/// list.
pub const OPS: [&str; 5] = [ADMIT, SET_ROLE, SUSPEND, REINSTATE, REMOVE];

/// The roles a member can hold. A guest is a row with no role and is not
/// something this admits.
pub const ROLES: [&str; 4] = ["owner", "manager", "editor", "viewer"];

/// Rebuilds the door on this replica. The runtime hands in one that runs
/// `refresh_access`; a test hands in one over its own engine.
pub type Refresh = Arc<dyn Fn(&str) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

pub struct MemberOps {
    /// The control database, which `ordo.members` is announced on.
    pub control: Pool,
    /// Which database a space's `members` rows are in.
    pub pools: Arc<dyn PoolResolver>,
    /// When present, a principal must exist in the deployment's directory
    /// before it can be admitted, so a typo does not become a dangling
    /// membership.
    pub directory: Option<Arc<dyn DirectoryStore>>,
    pub stream: Option<Arc<Recorder>>,
    pub refresh: Option<Refresh>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// Who is managed by which tenant (E6). Every admission tells it, so a
    /// managed person admitted to a second tenant becomes independent at
    /// that moment.
    pub managed: Option<crate::credential_ops::ManagedStore>,
}

/// What an admission wrote.
pub struct Admitted {
    pub member: String,
    pub principal: String,
    pub role: String,
}

fn refused(msg: &str) -> ExecError {
    ExecError::new(ErrorCode::TenantRefused, msg)
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

fn role_of(bound: &BoundParams) -> Result<String, ExecError> {
    let role = required(bound, "role")?;
    if ROLES.contains(&role) {
        Ok(role.to_string())
    } else {
        Err(refused(
            "a role is one of owner, manager, editor or viewer; a guest is a row with no role \
             and is not something a command admits",
        ))
    }
}

/// A member id or a principal id is spliced into a statement as a quoted
/// literal, so it is held to the shape the door can address.
fn well_formed(id: &str) -> bool {
    ordo_api::well_formed_space(id)
}

/// The columns a command reads back.
const READ: &str = r#""id", "principal", "role", "status""#;

impl MemberOps {
    /// The name a member row carries for `principal`: the display name the
    /// directory holds for the person, and the id only when there is none
    /// (no directory, an unknown id or an empty name). Every writer of a
    /// member row that is not handed the person's own words goes through
    /// here, so a person never reads back as an account id while the
    /// directory knows their name (25ba, 25be).
    pub fn display_name_of(&self, principal: &str) -> String {
        self.directory
            .as_ref()
            .and_then(|d| {
                d.principal(&PrincipalId(principal.to_string()))
                    .ok()
                    .flatten()
            })
            .map(|p| p.display.trim().to_string())
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| principal.to_string())
    }

    /// The database a space's `members` rows are in.
    pub async fn pool_of(&self, space: &str) -> Result<Pool, ExecError> {
        self.pool(space).await
    }

    /// `principal` was just admitted to `space`. If another tenant manages
    /// them, they stop being managed, on the record (`plan-enrolment.md`
    /// §4.2): from now on a reset by that tenant would reach into this one.
    pub async fn admitted(&self, space: &str, principal: &str) {
        let Some(managed) = &self.managed else {
            return;
        };
        let at = self.at();
        if let Ok(Some(was)) = managed.release_elsewhere(principal, space, at).await {
            if let Some(stream) = &self.stream {
                crate::enrolment::record(
                    stream,
                    &crate::enrolment::Act {
                        act: "identity-released",
                        detail: format!(
                            "principal={principal} was_managed_by={was} admitted_in={space} \
                             reason=admitted-to-a-second-tenant"
                        ),
                        by: None,
                        tenant: Some(&was),
                        deployment: true,
                        at,
                    },
                );
            }
        }
    }

    /// The clock, for acts recorded by a caller of this module.
    pub fn at(&self) -> u64 {
        (self.now)()
    }

    /// How many members a space has, of any status. Zero is a space nobody
    /// has claimed.
    pub async fn count_members(&self, space: &str) -> Result<i64, ExecError> {
        let pool = self.pool(space).await?;
        let rows = pool
            .rows(&format!(
                r#"SELECT count(*) FROM "members" WHERE "$space" = {} AND "kind" = 'principal'"#,
                sql_lit(space)
            ))
            .await
            .map_err(|e| {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
            })?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|c| c.parse().ok())
            .unwrap_or(0))
    }

    /// The member id a principal holds in a space, if any.
    pub async fn member_of(
        &self,
        space: &str,
        principal: &str,
    ) -> Result<Option<String>, ExecError> {
        if !well_formed(principal) {
            return Ok(None);
        }
        let pool = self.pool(space).await?;
        let rows = pool
            .rows(&format!(
                r#"SELECT "id" FROM "members"
                   WHERE "$space" = {} AND "principal" = {} AND "kind" = 'principal'"#,
                sql_lit(space),
                sql_lit(principal)
            ))
            .await
            .map_err(|e| {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
            })?;
        Ok(rows.first().and_then(|r| r.first()).and_then(|c| c.clone()))
    }

    /// The principal a member id names in a space. The door hands an op the
    /// member id, and a person is one principal whatever they are a member of,
    /// so anything that must count a person once (an approval) or name them
    /// the same in every stream resolves the id here first.
    pub async fn principal_of_member(
        &self,
        space: &str,
        member: &str,
    ) -> Result<Option<String>, ExecError> {
        if !well_formed(member) {
            return Ok(None);
        }
        let pool = self.pool(space).await?;
        let rows = pool
            .rows(&format!(
                r#"SELECT "principal" FROM "members"
                   WHERE "$space" = {} AND "id" = {} AND "kind" = 'principal'"#,
                sql_lit(space),
                sql_lit(member)
            ))
            .await
            .map_err(|e| {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
            })?;
        Ok(rows.first().and_then(|r| r.first()).and_then(|c| c.clone()))
    }

    /// The role and status a principal holds in a space, if it is a member.
    pub async fn state_of(
        &self,
        space: &str,
        principal: &str,
    ) -> Result<Option<(String, String)>, ExecError> {
        if !well_formed(principal) {
            return Ok(None);
        }
        let pool = self.pool(space).await?;
        let rows = pool
            .rows(&format!(
                r#"SELECT "role", "status" FROM "members"
                   WHERE "$space" = {} AND "principal" = {} AND "kind" = 'principal'"#,
                sql_lit(space),
                sql_lit(principal)
            ))
            .await
            .map_err(|e| {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
            })?;
        Ok(rows.first().map(|r| {
            let c = |i: usize| r.get(i).and_then(|c| c.clone()).unwrap_or_default();
            (c(0), c(1))
        }))
    }

    /// Every principal holding the owner role in a space, whatever their
    /// status. A succession names them as the departed owners when it opens.
    pub async fn owners_of(&self, space: &str) -> Result<Vec<String>, ExecError> {
        let pool = self.pool(space).await?;
        let rows = pool
            .rows(&format!(
                r#"SELECT "principal" FROM "members"
                   WHERE "$space" = {} AND "role" = 'owner' AND "kind" = 'principal'
                   ORDER BY "principal""#,
                sql_lit(space)
            ))
            .await
            .map_err(|e| {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
            })?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first().cloned().flatten())
            .collect())
    }

    /// Make an existing **active** member an owner, and answer their member id
    /// and the role they held. `None` when the principal is not an active
    /// member. Owner succession is the caller; it touches no credential and no
    /// session, and it does not rebuild the door (the caller does, once).
    pub async fn make_owner(
        &self,
        space: &str,
        principal: &str,
    ) -> Result<Option<(String, String)>, ExecError> {
        let Some((was, status)) = self.state_of(space, principal).await? else {
            return Ok(None);
        };
        if status != "active" {
            return Ok(None);
        }
        let pool = self.pool(space).await?;
        let rows = pool
            .rows(&format!(
                r#"UPDATE "members" SET "role" = 'owner', "rev" = "rev" + 1
                   WHERE "$space" = {} AND "principal" = {} AND "kind" = 'principal'
                     AND "status" = 'active'
                   RETURNING "id""#,
                sql_lit(space),
                sql_lit(principal)
            ))
            .await
            .map_err(|e| {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
            })?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.clone())
            .map(|id| (id, was)))
    }

    /// Suspend a recovered person's membership in a tenant that holds
    /// (`crate::recovery_hold`). `None` when they are not an active member,
    /// or when they are the tenant's **only active owner**: nobody would be
    /// left to release the hold. The owner rows are locked in the statement
    /// that writes, as the membership commands' own last-owner guard does.
    /// Does not rebuild the door; the caller does.
    pub async fn hold_member(
        &self,
        space: &str,
        principal: &str,
    ) -> Result<Option<String>, ExecError> {
        if !well_formed(principal) {
            return Ok(None);
        }
        let pool = self.pool(space).await?;
        let sql = format!(
            r#"WITH owners AS (
                 SELECT "id", "principal" FROM "members"
                 WHERE "$space" = {s} AND "role" = 'owner' AND "status" = 'active' FOR UPDATE)
               UPDATE "members" SET "status" = 'suspended', "rev" = "rev" + 1
               WHERE "$space" = {s} AND "principal" = {p} AND "kind" = 'principal'
                 AND "status" = 'active'
                 AND ("role" IS DISTINCT FROM 'owner'
                      OR EXISTS (SELECT 1 FROM owners WHERE "principal" <> {p}))
               RETURNING "id""#,
            s = sql_lit(space),
            p = sql_lit(principal),
        );
        let rows = pool.rows(&sql).await.map_err(|e| {
            ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
        })?;
        Ok(rows.first().and_then(|r| r.first()).and_then(|c| c.clone()))
    }

    async fn pool(&self, space: &str) -> Result<Pool, ExecError> {
        self.pools
            .resolve(&ordo_crypto::SpaceId(space.to_string()))
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string()))
    }

    /// Run a command by name.
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        match op {
            ADMIT => self.admit(space, bound, caller).await,
            SET_ROLE => self.set_role(space, bound, caller).await,
            SUSPEND => self.set_status(space, bound, caller, true).await,
            REINSTATE => self.set_status(space, bound, caller, false).await,
            REMOVE => self.remove(space, bound, caller).await,
            other => Err(ExecError::internal(format!(
                "`{other}` is not a membership command"
            ))),
        }
    }

    /// Admit a principal to a space, at a role. Shared with admission
    /// policy (E4), which admits on a sign-in instead of on a command.
    ///
    /// Returns `None` when the principal already holds a membership here.
    pub async fn admit_row(
        pool: &Pool,
        space: &str,
        principal: &str,
        role: &str,
        display: &str,
    ) -> Result<Option<Admitted>, ExecError> {
        let member = format!("mem_{}", random_principal_suffix());
        let sql = format!(
            r#"INSERT INTO "members"
               ("$space","id","rev","principal","displayName","kind","memberCount","role","status","joined")
               SELECT {s}, {m}, 1, {p}, {d}, 'principal', 1, {r}, 'active', CURRENT_DATE
               WHERE NOT EXISTS (SELECT 1 FROM "members" WHERE "$space" = {s} AND "principal" = {p})
               RETURNING "id""#,
            s = sql_lit(space),
            m = sql_lit(&member),
            p = sql_lit(principal),
            d = sql_lit(display),
            r = sql_lit(role),
        );
        let rows = pool.rows(&sql).await.map_err(|e| {
            ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
        })?;
        Ok((!rows.is_empty()).then(|| Admitted {
            member,
            principal: principal.to_string(),
            role: role.to_string(),
        }))
    }

    async fn admit(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let principal = required(bound, "principal")?;
        let role = role_of(bound)?;
        if !well_formed(principal) {
            return Err(refused("that is not a principal id this door can address"));
        }
        if let Some(dir) = &self.directory {
            let known = dir
                .principal(&PrincipalId(principal.to_string()))
                .map_err(|_| ExecError::internal("the directory could not be read"))?;
            if known.is_none() {
                return Err(ExecError::new(
                    ErrorCode::NotFound,
                    "this deployment knows no such principal",
                ));
            }
        }
        let named;
        let display = match text(bound, "display") {
            Some(d) => d,
            None => {
                named = self.display_name_of(principal);
                &named
            }
        };
        let pool = self.pool(space).await?;
        let Some(a) = Self::admit_row(&pool, space, principal, &role, display).await? else {
            return Err(ExecError::new(
                ErrorCode::Duplicate,
                "that principal is already a member of this space",
            ));
        };
        self.note(
            space,
            caller,
            "member-admitted",
            format!(
                "member={} principal={} role={} way=administrator",
                a.member, a.principal, a.role
            ),
        );
        self.admitted(space, &a.principal).await;
        let door = self.rebuilt(space).await;
        Ok(answer(
            &a.member,
            &a.principal,
            &a.role,
            "active",
            "admitted",
            door,
        ))
    }

    /// The row as it stands, for the command that is about to change it.
    async fn read(
        &self,
        pool: &Pool,
        space: &str,
        member: &str,
    ) -> Result<(String, String, String), ExecError> {
        if !well_formed(member) {
            return Err(not_found());
        }
        let rows = pool
            .rows(&format!(
                r#"SELECT {READ} FROM "members"
                   WHERE "$space" = {} AND "id" = {} AND "kind" = 'principal'"#,
                sql_lit(space),
                sql_lit(member)
            ))
            .await
            .map_err(|e| {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
            })?;
        let r = rows.first().ok_or_else(not_found)?;
        let c = |i: usize| r.get(i).and_then(|c| c.clone()).unwrap_or_default();
        Ok((c(1), c(2), c(3)))
    }

    /// The condition that keeps the last active owner. `owners` is the CTE
    /// that locks the owner rows, so two owners acting on each other at the
    /// same moment are serialised.
    fn keeps_an_owner(space: &str, member: &str) -> (String, String) {
        let cte = format!(
            r#"WITH owners AS (
                 SELECT "id" FROM "members"
                 WHERE "$space" = {} AND "role" = 'owner' AND "status" = 'active' FOR UPDATE)"#,
            sql_lit(space)
        );
        let cond = format!(
            r#"EXISTS (SELECT 1 FROM owners WHERE "id" <> {})"#,
            sql_lit(member)
        );
        (cte, cond)
    }

    async fn set_role(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let member = required(bound, "member")?;
        let role = role_of(bound)?;
        let pool = self.pool(space).await?;
        let (principal, was, status) = self.read(&pool, space, member).await?;
        let (cte, other_owner) = Self::keeps_an_owner(space, member);
        let sql = format!(
            r#"{cte}
               UPDATE "members" SET "role" = {r}, "rev" = "rev" + 1
               WHERE "$space" = {s} AND "id" = {m} AND "kind" = 'principal'
                 AND ("role" IS DISTINCT FROM 'owner' OR {r} = 'owner'
                      OR "status" <> 'active' OR {other_owner})
               RETURNING "id""#,
            r = sql_lit(&role),
            s = sql_lit(space),
            m = sql_lit(member),
        );
        let rows = pool.rows(&sql).await.map_err(|e| {
            ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
        })?;
        if rows.is_empty() {
            return Err(last_owner());
        }
        self.note(
            space,
            caller,
            "member-role-changed",
            format!("member={member} principal={principal} from={was} to={role}"),
        );
        let door = self.rebuilt(space).await;
        Ok(answer(
            member,
            &principal,
            &role,
            &status,
            "role changed",
            door,
        ))
    }

    async fn set_status(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
        suspend: bool,
    ) -> Result<ExecResult, ExecError> {
        let member = required(bound, "member")?;
        let pool = self.pool(space).await?;
        let (principal, role, _) = self.read(&pool, space, member).await?;
        let (to, guard) = if suspend {
            let (cte, other_owner) = Self::keeps_an_owner(space, member);
            (
                "suspended",
                (
                    cte,
                    format!(
                        r#" AND ("role" IS DISTINCT FROM 'owner' OR "status" <> 'active' OR {other_owner})"#
                    ),
                ),
            )
        } else {
            ("active", (String::new(), String::new()))
        };
        let sql = format!(
            r#"{cte}
               UPDATE "members" SET "status" = {to}, "rev" = "rev" + 1
               WHERE "$space" = {s} AND "id" = {m} AND "kind" = 'principal'{cond}
               RETURNING "id""#,
            cte = guard.0,
            cond = guard.1,
            to = sql_lit(to),
            s = sql_lit(space),
            m = sql_lit(member),
        );
        let rows = pool.rows(&sql).await.map_err(|e| {
            ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
        })?;
        if rows.is_empty() {
            return Err(last_owner());
        }
        if let Some(stream) = &self.stream {
            stream.observe(
                Observation {
                    space: Some(ordo_commandlog::SpaceId(space.to_string())),
                    subject: Subject::member(caller),
                    op: format!("member.{}", if suspend { "suspend" } else { "reinstate" }),
                    kind: if suspend {
                        EventKind::MembershipSuspended
                    } else {
                        EventKind::MembershipReactivated
                    },
                    at: ordo_commandlog::Timestamp((self.now)()),
                    correlation: ordo_secevent::correlation::mint(),
                },
                Relationship::Member,
            );
        }
        let door = self.rebuilt(space).await;
        Ok(answer(
            member,
            &principal,
            &role,
            to,
            if suspend { "suspended" } else { "reinstated" },
            door,
        ))
    }

    async fn remove(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let member = required(bound, "member")?;
        let pool = self.pool(space).await?;
        let (principal, role, status) = self.read(&pool, space, member).await?;
        let (cte, other_owner) = Self::keeps_an_owner(space, member);
        let sql = format!(
            r#"{cte}
               DELETE FROM "members"
               WHERE "$space" = {s} AND "id" = {m} AND "kind" = 'principal'
                 AND ("role" IS DISTINCT FROM 'owner' OR "status" <> 'active' OR {other_owner})
               RETURNING "id""#,
            s = sql_lit(space),
            m = sql_lit(member),
        );
        let rows = match pool.rows(&sql).await {
            Ok(rows) => rows,
            // A member other rows point at cannot be deleted without
            // orphaning them. The answer names the way out.
            Err(e) if e.is_foreign_key_violation() => {
                return Err(ExecError::new(
                    ErrorCode::HasReferrers,
                    "other rows in this space still refer to that member; suspend them \
                     instead, or move those rows first",
                ));
            }
            Err(e) => {
                return Err(ExecError::of_fault(
                    e.class(),
                    ErrorCode::SpaceUnavailable,
                    e.to_string(),
                ))
            }
        };
        if rows.is_empty() {
            return Err(last_owner());
        }
        self.note(
            space,
            caller,
            "member-removed",
            format!("member={member} principal={principal} role={role} status={status}"),
        );
        let door = self.rebuilt(space).await;
        Ok(answer(
            member, &principal, &role, "removed", "removed", door,
        ))
    }

    /// One tenant-scoped act on the record.
    pub fn note(&self, space: &str, caller: &str, act: &str, detail: String) {
        if let Some(stream) = &self.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act,
                    detail: format!("{detail} by={caller}"),
                    by: Some(caller),
                    tenant: Some(space),
                    deployment: false,
                    at: (self.now)(),
                },
            );
        }
    }

    /// Rebuild the door here, then tell every replica to. Returns what
    /// happened, in words the answer can carry.
    pub async fn rebuilt(&self, space: &str) -> &'static str {
        let announced = crate::deploy::announce_members(&self.control, space)
            .await
            .is_ok();
        let here = match &self.refresh {
            Some(refresh) => refresh(space).await.is_ok(),
            None => false,
        };
        match (here, announced) {
            (true, true) => "rebuilt here, announced to every replica",
            (true, false) => "rebuilt here, and the announcement failed: other replicas follow at their next refresh",
            (false, true) => "announced to every replica, and this one follows at its next refresh",
            (false, false) => "not rebuilt: every replica follows at its next refresh",
        }
    }
}

fn not_found() -> ExecError {
    ExecError::new(ErrorCode::NotFound, "this space has no such member")
}

fn last_owner() -> ExecError {
    refused(
        "that would leave the space with no active owner, which nothing can repair from \
         inside. Make another member an owner first",
    )
}

fn answer(
    member: &str,
    principal: &str,
    role: &str,
    status: &str,
    outcome: &str,
    door: &str,
) -> ExecResult {
    ExecResult {
        position: None,
        rows: vec![json!({
            "member": member,
            "principal": principal,
            "role": role,
            "status": status,
            "outcome": outcome,
            "door": door,
        })],
        next_cursor: None,
    }
}
