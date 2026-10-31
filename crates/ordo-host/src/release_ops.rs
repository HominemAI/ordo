//! Leaving a tenant: `std.space.leave` (checklist 27aq, `plan-member-release.md`
//! §3, owner rulings of 2026-10-05).
//!
//! # What the owner ruled, and what that makes this
//!
//! Leaving exists as a mechanism, and whether it is allowed is the app's
//! choice: the program's `release member` line. With no line, members are
//! **locked**. There is no tenant override built in: a tenant can still
//! remove a member (`std.space.remove`), and if the app's rule includes
//! something a tenant can do, that is the app's rule. So this module is the
//! rule evaluated, and nothing more.
//!
//! # The order a leave is decided in
//!
//! One transaction, with the seat locked:
//!
//! 1. the seat is the caller's, a member with a role, and not already left;
//! 2. **the platform's own conditions**, whatever the program writes: the
//!    last active owner cannot leave, and neither can the owner of a tenant
//!    that is their own (`last_owner`);
//! 3. **the rule**: no line or `locked` is `release_locked`; a `when` rule
//!    whose conditions do not hold is `release_held`, naming each condition as
//!    written and never a row;
//! 4. **the effects**: the program's `remove` and `reassign` lines, then the
//!    platform's (the seat's acl rows and group memberships go);
//! 5. **the seat**: kept, `status = left`, no role, the principal unlinked.
//!    The name is kept (owner ruling 2026-10-06); the app developer may set
//!    `ORDO_MEMBER_LEFT_NAME` to replace it on leave. Rows that ref it keep
//!    pointing at it unless an effect said otherwise.
//!
//! Steps 1 to 3 are reads, made here. Steps 4 and 5 are the write protocol's
//! (`Fixture::run_release_on`), called inside the same transaction through
//! `Executor::platform_write`.
//!
//! After the commit: the person's share grants in the tenant are revoked, a
//! managed identity becomes independent (§3.3), the act is recorded, and the
//! door is rebuilt here and announced to every replica.
//!
//! # The leave is one command group
//!
//! Every row the leave changes (the app's removed and reassigned rows, the
//! seat's acl rows, its group links and their groups' counts, and the seat)
//! is locked, imaged and written by the protocol, and the whole leave is one
//! group in the tenant's chain, appended in the transaction that commits it.
//! So it is revertible as a unit, triggers and live subscriptions see it as
//! they see any write, and `rev` moves the normal way. The act is also on
//! the tenant's security stream, with each effect's count.
//!
//! **Guards.** The release rule is the app's authority for its effects, so
//! entity guards are not re-evaluated against the leaver; the reasoning is
//! on `Fixture::run_release_on`. Structure still holds: a row other rows
//! need is not deleted, and the leave is refused `release_held`.
//!
//! **What a revert of the group does not undo** is what lives outside the
//! tenant's rows: the share grants revoked and a managed identity released
//! after the commit.

use std::sync::Arc;

use std::sync::Mutex;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_conformance::write::{ReleaseStep, WriteDb};
use ordo_conformance::Fixture;
use ordo_exec::{sql_lit, PoolError, TxConn};
use ordo_identity::grants::GrantRegistry;
use ordo_identity::principal::{PrincipalId, SpaceId, Timestamp};
use ordo_syntax::{ReassignTarget, ReleaseEffect, ReleaseMode};
use serde_json::{json, Value};

use crate::member_ops::MemberOps;

/// `std.space.leave`: the caller releases their own seat in the bound tenant.
pub const LEAVE: &str = "std.space.leave";

pub const OPS: [&str; 1] = [LEAVE];

/// The app developer's setting for the name a left seat reads as.
///
/// Unset or empty, the seat **keeps its name**: the default the owner ruled
/// on 2026-10-06 (*"Default should be to leave it ... This is a dev
/// choice"*). The seat reads `status = left`, and the app decides how to
/// show it, for example `Dana (Deactivated)`. Set, its text replaces the
/// seat's `displayName` when the seat leaves. History keeps the name either
/// way: the seat's before-image is in the leave's command group.
pub const LEFT_NAME_ENV: &str = "ORDO_MEMBER_LEFT_NAME";

/// The longest replacement name, in characters, the same bound a tenant's
/// own name has.
pub const LEFT_NAME_MAX: usize = 256;

/// Read [`LEFT_NAME_ENV`]. `Ok(None)` keeps names; a value longer than
/// [`LEFT_NAME_MAX`] refuses the boot rather than being cut.
pub fn left_name_from_env() -> Result<Option<String>, String> {
    left_name_from(std::env::var(LEFT_NAME_ENV).ok().as_deref())
}

/// [`left_name_from_env`] over a given value, so the reading is testable.
pub fn left_name_from(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if v.chars().count() > LEFT_NAME_MAX {
        return Err(format!(
            "the replacement name is longer than {LEFT_NAME_MAX} characters; unset it to keep \
             each leaver's name"
        ));
    }
    Ok(Some(v.to_string()))
}

/// The census line for the setting in use.
pub fn describe_left_name(left_name: Option<&str>) -> String {
    match left_name {
        None => "a member who leaves keeps their name (the default)".to_string(),
        Some(n) => format!("a member who leaves is renamed `{n}`, set by {LEFT_NAME_ENV}"),
    }
}

/// Is this status a seat that is no membership: one that left, or one whose
/// person then deleted their account?
pub fn is_gone(status: &str) -> bool {
    status == "left" || status == "deleted"
}

/// Which platform ops every member may call, with any role. The door grants
/// them to all four roles; a guest (no role) keeps `/auth/share/leave`.
pub fn any_member(name: &str) -> bool {
    OPS.contains(&name)
}

pub struct ReleaseOps {
    /// The seat's database, the record, and the door's rebuild.
    pub members: Arc<MemberOps>,
    /// The space resolver every op call goes through: the program being
    /// served, cut to the tenant, its executor and its chain. Read on every
    /// call, so a deploy that changes the rule applies from the next leave,
    /// and the leave's group joins the same chain an op's does.
    pub spaces: Arc<crate::spaces::Spaces>,
    /// Where share grants are revoked. `None` revokes none.
    pub grants: Option<Arc<dyn GrantRegistry>>,
    /// The app developer's replacement for a leaver's name
    /// ([`LEFT_NAME_ENV`]). `None`, the default, keeps the name.
    pub left_name: Option<String>,
}

/// What stands between a member and leaving one tenant. Shared by the leave
/// and by the account deletion's refusal body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bar {
    /// Nothing: the rule clears.
    Open,
    /// The tenant is the person's own (`tenants::own_of`).
    Own,
    /// The last active owner.
    LastOwner,
    /// A seat with no role: `/auth/share/leave` closes it.
    Guest,
    /// No release line, or `release member locked`.
    Locked,
    /// The rule's conditions, as written, that do not hold.
    Held(Vec<String>),
}

impl Bar {
    pub fn word(&self) -> &'static str {
        match self {
            Bar::Open => "open",
            Bar::Own => "own",
            Bar::LastOwner => "last_owner",
            Bar::Guest => "guest",
            Bar::Locked => "locked",
            Bar::Held(_) => "held",
        }
    }

    /// The refusal a leave answers with when this bar stands.
    fn refusal(&self) -> ExecError {
        let msg = match self {
            Bar::Open => return ExecError::internal("an open bar is not a refusal"),
            Bar::Own => "last_owner: this tenant is your own; delete it, or hand it on with \
                         std.space.succeedOwner, before leaving"
                .to_string(),
            Bar::LastOwner => "last_owner: you are this tenant's last active owner; make \
                               another member an owner first"
                .to_string(),
            Bar::Guest => "guest_seat: a guest seat is closed with /auth/share/leave".to_string(),
            Bar::Locked => "release_locked: this app does not let members leave a tenant on \
                            their own; the tenant can remove you"
                .to_string(),
            Bar::Held(atoms) => format!(
                "release_held: this app's release rule holds your seat: {}",
                atoms.join("; ")
            ),
        };
        ExecError::new(ErrorCode::TenantRefused, msg)
    }
}

/// The seat as the leave found it.
struct Seat {
    principal: String,
    role: Option<String>,
    status: String,
}

fn unavailable(e: impl crate::Faulty) -> ExecError {
    crate::op_store_unavailable(e)
}

fn q(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn cell(rows: &[Vec<Option<String>>]) -> Option<String> {
    rows.first().and_then(|r| r.first()).and_then(|c| c.clone())
}

fn truthy(rows: &[Vec<Option<String>>]) -> bool {
    matches!(cell(rows).as_deref(), Some("t") | Some("true"))
}

/// What one attempt at a leave came to, carried out of the transaction's
/// blocking thread beside the protocol's `Result<Value, String>`.
enum Verdict {
    /// The seat had already left: nothing written.
    AlreadyLeft,
    /// No principal seat with this id.
    NoSeat,
    /// Something stands in the way: nothing written.
    Refused(Bar),
    /// The seat moved under the leave (its principal changed between the
    /// read outside the transaction and the lock inside it).
    Moved,
    /// Written: the rule that cleared, and how many of the steps handed to
    /// the protocol were the program's.
    Released {
        rule: &'static str,
        program: Vec<String>,
    },
}

/// Why the closure stopped without writing. The protocol's error channel is
/// a string, so the verdict travels in its own slot and this is what the
/// string says.
const STOPPED: &str = "release_stopped";

/// An open transaction, for the reads `bar_for` makes. The same bridge the
/// executor's `BlockingTx` is, read-only, and only ever built inside
/// `spawn_blocking`.
struct ReadTx {
    tx: TxConn,
    handle: tokio::runtime::Handle,
}

impl WriteDb for ReadTx {
    fn rows(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        self.handle
            .block_on(self.tx.rows(sql))
            .map_err(|e| e.to_string())
    }
}

/// The seat `member` holds in `space`, locked for the transaction.
fn seat(db: &dyn WriteDb, space: &str, member: &str) -> Result<Option<Seat>, String> {
    if !ordo_api::well_formed_space(member) {
        return Ok(None);
    }
    let rows = db.rows(&format!(
        r#"SELECT "principal", "role", "status" FROM "members"
           WHERE "$space" = {} AND "id" = {} AND "kind" = 'principal' FOR UPDATE"#,
        sql_lit(space),
        sql_lit(member)
    ))?;
    Ok(rows.first().map(|r| Seat {
        principal: r.first().cloned().flatten().unwrap_or_default(),
        role: r.get(1).cloned().flatten().filter(|s| !s.is_empty()),
        status: r.get(2).cloned().flatten().unwrap_or_default(),
    }))
}

/// What stands in the way of `member` leaving `space`, read inside the
/// transaction. The platform's conditions first, so the answer names the
/// first thing in the way. `own` is whether the tenant is the person's own,
/// read from the control database before the transaction.
fn assess(
    db: &dyn WriteDb,
    f: &Fixture,
    space: &str,
    member: &str,
    seat: &Seat,
    own: bool,
) -> Result<Bar, String> {
    let Some(role) = &seat.role else {
        return Ok(Bar::Guest);
    };
    if own {
        return Ok(Bar::Own);
    }
    if role == "owner" {
        let others = db.rows(&format!(
            r#"SELECT count(*) FROM (SELECT "id" FROM "members"
               WHERE "$space" = {} AND "role" = 'owner' AND "status" = 'active'
                 AND "kind" = 'principal' AND "id" <> {} FOR UPDATE) o"#,
            sql_lit(space),
            sql_lit(member)
        ))?;
        if cell(&others).as_deref() == Some("0") {
            return Ok(Bar::LastOwner);
        }
    }
    let Some(rule) = &f.schema.release else {
        return Ok(Bar::Locked);
    };
    match &rule.mode {
        ReleaseMode::Open { .. } => Ok(Bar::Open),
        ReleaseMode::Locked { .. } => Ok(Bar::Locked),
        ReleaseMode::When(cond) => {
            let mut failed: Vec<String> = Vec::new();
            for conj in &cond.disjuncts {
                let mut all = true;
                for atom in conj {
                    let query = ordo_sema::release_query(
                        &f.schema,
                        &atom.entity.name,
                        &atom.inner,
                        member,
                        "id",
                    )
                    .map_err(|e| format!("internal: release rule: {e}"))?;
                    let plan = ordo_plan::plan(&query, &ordo_plan::Space::Bound(space.to_string()))
                        .map_err(|e| format!("internal: release rule: {e}"))?;
                    let any = truthy(&db.rows(&format!(
                        "SELECT EXISTS ({})",
                        plan.sql.trim_end_matches(';')
                    ))?);
                    if any == atom.negated {
                        all = false;
                        let text = ordo_schema::render_release_atom(atom);
                        if !failed.contains(&text) {
                            failed.push(text);
                        }
                    }
                }
                if all {
                    return Ok(Bar::Open);
                }
            }
            Ok(Bar::Held(failed))
        }
    }
}

/// The member a reassignment target names: an active member with a role,
/// other than the leaver, and exactly one of them.
fn target(
    db: &dyn WriteDb,
    f: &Fixture,
    space: &str,
    seat: &str,
    t: &ReassignTarget,
) -> Result<Option<String>, String> {
    let s = sql_lit(space);
    let m = sql_lit(seat);
    let rows = match t {
        ReassignTarget::Owner { .. } => db.rows(&format!(
            r#"SELECT "id" FROM "members"
               WHERE "$space" = {s} AND "role" = 'owner' AND "status" = 'active'
                 AND "kind" = 'principal' AND "id" <> {m}
               ORDER BY "joined", "id" LIMIT 1"#
        ))?,
        ReassignTarget::Lookup {
            entity,
            inner,
            field,
            ..
        } => {
            let query = ordo_sema::release_query(&f.schema, &entity.name, inner, seat, &field.name)
                .map_err(|e| format!("internal: release rule: {e}"))?;
            let plan = ordo_plan::plan(&query, &ordo_plan::Space::Bound(space.to_string()))
                .map_err(|e| format!("internal: release rule: {e}"))?;
            let rows = db.rows(&format!(
                r#"SELECT DISTINCT v.{col} FROM ({sub}) v
                   JOIN "members" mm ON mm."id" = v.{col} AND mm."$space" = {s}
                   WHERE mm."status" = 'active' AND mm."role" IS NOT NULL
                     AND mm."kind" = 'principal' AND v.{col} <> {m}
                   LIMIT 2"#,
                col = q(&field.name),
                sub = plan.sql.trim_end_matches(';'),
            ))?;
            // Two people named is no one named: the rule cannot say which,
            // and guessing would hand rows to the wrong person.
            if rows.len() == 1 {
                rows
            } else {
                Vec::new()
            }
        }
    };
    Ok(cell(&rows))
}

/// The program's `remove` and `reassign` lines as steps for the protocol,
/// each `reassign` resolved to the member it names. Targets are resolved
/// before any effect runs, against the tenant as the leave found it.
///
/// A `reassign` that names nobody while rows still point at the seat holds
/// the seat: `Err` carries the refusal's text. One that names nobody with no
/// rows to move is no step at all.
fn program_steps(
    db: &dyn WriteDb,
    f: &Fixture,
    space: &str,
    seat: &str,
) -> Result<Result<Vec<ReleaseStep>, String>, String> {
    let mut out = Vec::new();
    let Some(rule) = &f.schema.release else {
        return Ok(Ok(out));
    };
    for effect in &rule.effects {
        match effect {
            ReleaseEffect::Remove { entity, field, .. } => out.push(ReleaseStep::Remove {
                entity: entity.name.clone(),
                field: field.name.clone(),
            }),
            ReleaseEffect::Reassign {
                entity, field, to, ..
            } => {
                let mut chosen = None;
                for t in to {
                    chosen = target(db, f, space, seat, t)?;
                    if chosen.is_some() {
                        break;
                    }
                }
                match chosen {
                    Some(to) => out.push(ReleaseStep::Reassign {
                        entity: entity.name.clone(),
                        field: field.name.clone(),
                        to,
                    }),
                    None => {
                        let any = db.rows(&format!(
                            r#"SELECT 1 FROM {} WHERE "$space" = {} AND {} = {} LIMIT 1"#,
                            q(&entity.name),
                            sql_lit(space),
                            q(&field.name),
                            sql_lit(seat)
                        ))?;
                        if !any.is_empty() {
                            return Ok(Err(format!(
                                "release_held: this app's release rule reassigns {}.{} and \
                                 names nobody to reassign it to",
                                entity.name, field.name
                            )));
                        }
                    }
                }
            }
        }
    }
    Ok(Ok(out))
}

/// What leaving always does beside the program's lines: the seat's rows in
/// every acl link entity go. (Group memberships go through the membership
/// surface, inside the protocol's release.)
fn platform_steps(f: &Fixture) -> Vec<ReleaseStep> {
    f.schema
        .entities
        .iter()
        .filter(|e| ordo_schema::acl_link_root(&f.schema, &e.name).is_some())
        .filter(|e| e.field("member").is_some())
        .map(|e| ReleaseStep::Remove {
            entity: e.name.clone(),
            field: "member".into(),
        })
        .collect()
}

/// The words a summary from the protocol becomes in the answer.
fn effects_of(summaries: &Value, program: &[String]) -> Value {
    let mut out = serde_json::Map::new();
    let rows = summaries.as_array().cloned().unwrap_or_default();
    let mut access = 0u64;
    let mut groups = 0u64;
    for (i, s) in rows.iter().enumerate() {
        let entity = s["entity"].as_str().unwrap_or("");
        let field = s["field"].as_str().unwrap_or("");
        if let Some(word) = program.get(i) {
            if word == "remove" {
                out.insert(format!("removed {entity}.{field}"), s["deleted"].clone());
            } else {
                out.insert(
                    format!("reassigned {entity}.{field}"),
                    json!({ "to": s["to"], "rows": s["updated"] }),
                );
            }
        } else if entity == "groupMembers" {
            groups += s["deleted"].as_u64().unwrap_or(0);
        } else if entity != "members" {
            access += s["deleted"].as_u64().unwrap_or(0);
        }
    }
    out.insert("accessRows".into(), json!(access));
    out.insert("groups".into(), json!(groups));
    Value::Object(out)
}

impl ReleaseOps {
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        _bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        match op {
            LEAVE => self.leave(space, caller).await,
            other => Err(ExecError::internal(format!(
                "`{other}` is not a release command"
            ))),
        }
    }

    /// Is `space` the person's own tenant? A deployment with no tenant
    /// registry has no own tenants.
    async fn own_tenant(&self, space: &str, principal: &str) -> Result<bool, ExecError> {
        let control = &self.members.control;
        if !crate::registrations::table_exists(control, "$spaces")
            .await
            .map_err(unavailable)?
        {
            return Ok(false);
        }
        Ok(crate::tenants::own_of(control, principal)
            .await
            .map_err(unavailable)?
            .is_some_and(|t| t.id == space))
    }

    /// What stands between the person and leaving `space`, without acting.
    /// `None` when they hold no seat there that is not already left.
    pub async fn bar_for(&self, space: &str, principal: &str) -> Result<Option<Bar>, ExecError> {
        let Some(member) = self.members.member_of(space, principal).await? else {
            return Ok(None);
        };
        let own = self.own_tenant(space, principal).await?;
        let bound = self.spaces.resolve(space).await.map_err(unavailable)?;
        let f = bound.view.fixture.clone();
        let tx = bound.view.exec.pool.begin().await.map_err(unavailable)?;
        let handle = tokio::runtime::Handle::current();
        let space = space.to_string();
        let (out, tx) = tokio::task::spawn_blocking(move || {
            let db = ReadTx { tx, handle };
            let out = match seat(&db, &space, &member) {
                Ok(Some(s)) if !is_gone(&s.status) => {
                    assess(&db, &f, &space, &member, &s, own).map(Some)
                }
                Ok(_) => Ok(None),
                Err(e) => Err(e),
            };
            (out, db.tx)
        })
        .await
        .map_err(|e| ExecError::internal(format!("release read: {e}")))?;
        let _ = tx.rollback().await;
        out.map_err(unavailable)
    }

    /// `std.space.leave`.
    pub async fn leave(&self, space: &str, caller: &str) -> Result<ExecResult, ExecError> {
        // Outside the transaction, from the control database: whether this
        // is the person's own tenant. Checked again inside against the
        // locked seat, so a seat whose principal moved meanwhile is not
        // decided on a stale answer.
        let Some(principal) = self.members.principal_of_member(space, caller).await? else {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "this tenant has no such member",
            ));
        };
        let own = self.own_tenant(space, &principal).await?;
        let bound = self.spaces.resolve(space).await.map_err(unavailable)?;
        let f = bound.view.fixture.clone();

        // Retried on a serialization conflict, from scratch: the decision
        // and the write are one attempt, so a retry decides again.
        let mut tries = 0;
        let (summaries, verdict) = loop {
            tries += 1;
            let slot: Arc<Mutex<Option<Verdict>>> = Arc::new(Mutex::new(None));
            // Whether the release's writes were refused by a foreign key or
            // a unique constraint, from the database's typed error
            // (`WriteDb::refused_by_constraint`).
            let held = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let work = {
                let (f, slot, held) = (f.clone(), slot.clone(), held.clone());
                let (space, caller, principal) =
                    (space.to_string(), caller.to_string(), principal.clone());
                let left_name = self.left_name.clone();
                move |db: &dyn WriteDb, log: &mut ordo_conformance::WriteLog| {
                    let stop = |v: Verdict| {
                        *slot.lock().expect("verdict") = Some(v);
                        Err(STOPPED.to_string())
                    };
                    let Some(s) = seat(db, &space, &caller)? else {
                        return stop(Verdict::NoSeat);
                    };
                    // Idempotent: a second leave from a seat that has left
                    // answers the same, and changes nothing.
                    if is_gone(&s.status) {
                        return stop(Verdict::AlreadyLeft);
                    }
                    if s.principal != principal {
                        return stop(Verdict::Moved);
                    }
                    let bar = assess(db, &f, &space, &caller, &s, own)?;
                    if bar != Bar::Open {
                        return stop(Verdict::Refused(bar));
                    }
                    let rule = match f.schema.release.as_ref().map(|r| &r.mode) {
                        Some(ReleaseMode::Open { .. }) => "open",
                        _ => "rule",
                    };
                    let mut steps = match program_steps(db, &f, &space, &caller)? {
                        Ok(steps) => steps,
                        Err(held) => return Err(held),
                    };
                    let program = steps
                        .iter()
                        .map(|s| match s {
                            ReleaseStep::Remove { .. } => "remove".to_string(),
                            ReleaseStep::Reassign { .. } => "reassign".to_string(),
                        })
                        .collect();
                    steps.extend(platform_steps(&f));
                    let out = f
                        .run_release_on(db, log, &caller, &steps, left_name.as_deref())
                        .inspect_err(|_| {
                            if db.refused_by_constraint() {
                                held.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                        })?;
                    *slot.lock().expect("verdict") = Some(Verdict::Released { rule, program });
                    Ok(out)
                }
            };
            let result = bound
                .view
                .exec
                .platform_write(bound.log.clone(), work)
                .await;
            let verdict = slot.lock().expect("verdict").take();
            match (result, verdict) {
                (Ok(v), Some(verdict @ Verdict::Released { .. })) => break (v, verdict),
                (Ok(_), _) => {
                    return Err(ExecError::internal("a leave wrote with no verdict"));
                }
                // Stopped before writing: the verdict is the answer.
                (Err(_), Some(verdict)) if !matches!(verdict, Verdict::Released { .. }) => {
                    break (Value::Null, verdict)
                }
                // Anything else did not commit, a `Released` slot included:
                // the protocol finished and the append or the commit did not.
                (Err(ordo_exec::ExecFail::Pool(PoolError::Serialization(_))), _) if tries < 3 => {
                    continue;
                }
                (Err(ordo_exec::ExecFail::Compile(m)), _) => {
                    let constraint = held.load(std::sync::atomic::Ordering::Relaxed);
                    return Err(refusal_of(&m, constraint));
                }
                (Err(e), _) => return Err(unavailable(e)),
            }
        };

        let (rule, program) = match verdict {
            Verdict::Released { rule, program } => (rule, program),
            Verdict::AlreadyLeft => {
                return Ok(answer(caller, "left", "already left", json!({}), ""));
            }
            Verdict::NoSeat => {
                return Err(ExecError::new(
                    ErrorCode::NotFound,
                    "this tenant has no such member",
                ));
            }
            Verdict::Moved => {
                return Err(unavailable(
                    "this seat changed while the leave was deciding; ask again",
                ));
            }
            Verdict::Refused(bar) => {
                self.members.note(
                    space,
                    caller,
                    "member-leave-refused",
                    format!("member={caller} bar={}", bar.word()),
                );
                return Err(bar.refusal());
            }
        };

        let grants = self.revoke_grants(space, &principal).await;
        let identity = self.settle_identity(space, &principal).await;
        let mut all = effects_of(&summaries, &program);
        all["shareGrants"] = json!(grants);
        if let Some(id) = &identity {
            all["identity"] = json!(id);
        }
        self.members.note(
            space,
            caller,
            "member-released",
            format!(
                "member={caller} principal={principal} by=self rule={rule} effects={}",
                all
            ),
        );
        let door = self.members.rebuilt(space).await;
        Ok(answer(caller, "left", rule, all, door))
    }

    /// The person's live share grants in the tenant, revoked. The door
    /// already refuses them once the seat is gone; this keeps the registry
    /// telling the truth.
    async fn revoke_grants(&self, space: &str, principal: &str) -> usize {
        let Some(grants) = &self.grants else {
            return 0;
        };
        let ispace = SpaceId(space.to_string());
        let now = Timestamp(self.members.at());
        let Ok(held) = grants
            .grants_of(&ispace, &PrincipalId(principal.to_string()))
            .await
        else {
            return 0;
        };
        let mut n = 0;
        let mut seen = Vec::new();
        for rec in held
            .iter()
            .filter(|r| r.id.0.starts_with(crate::share_ops::GRANT_PREFIX) && r.is_live(now))
        {
            if seen.contains(&rec.id) {
                continue;
            }
            seen.push(rec.id.clone());
            if grants.revoke(&ispace, &rec.id, now).await.is_ok() {
                n += 1;
            }
        }
        n
    }

    /// §3.3: a person this tenant made, leaving it, follows E7's rule. Signed
    /// in at least once, they become independent; never signed in, the
    /// identity goes with the seat. A self-leave has signed in by definition,
    /// so in practice this releases.
    async fn settle_identity(&self, space: &str, principal: &str) -> Option<&'static str> {
        let control = &self.members.control;
        if !crate::registrations::table_exists(control, "$managed")
            .await
            .ok()?
        {
            return None;
        }
        let rows = control
            .rows(&format!(
                r#"SELECT 1 FROM "$managed"
                   WHERE "principal" = {} AND "tenant" = {} AND "released_at" IS NULL"#,
                sql_lit(principal),
                sql_lit(space)
            ))
            .await
            .ok()?;
        if rows.is_empty() {
            return None;
        }
        let at = self.members.at();
        let signed_in = crate::registrations::table_exists(control, "$signedin")
            .await
            .ok()?
            && !control
                .rows(&format!(
                    r#"SELECT 1 FROM "$signedin" WHERE "principal" = {}"#,
                    sql_lit(principal)
                ))
                .await
                .ok()?
                .is_empty();
        let (act, reason) = if signed_in {
            control
                .rows(&format!(
                    r#"UPDATE "$managed" SET "released_at" = {at}
                       WHERE "principal" = {} AND "released_at" IS NULL"#,
                    sql_lit(principal)
                ))
                .await
                .ok()?;
            ("identity-released", "left-the-tenant")
        } else {
            // The one identity removal path (R6), so a leave and a tenant
            // deletion cannot disagree about what "removed" means.
            crate::identity_removal::remove(control, principal)
                .await
                .ok()?;
            ("identity-removed", "never-signed-in")
        };
        if let Some(stream) = &self.members.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act,
                    detail: format!("principal={principal} reason={reason} tenant={space}"),
                    by: Some(principal),
                    tenant: Some(space),
                    deployment: true,
                    at,
                },
            );
        }
        Some(if signed_in { "released" } else { "removed" })
    }
}

/// The refusal a protocol error from a leave becomes. The protocol speaks in
/// `class: message` strings; the ones a release rule can cause are the
/// tenant's refusal, and the rest are the database being unavailable. A
/// foreign key or unique constraint that refused the release's writes is
/// `constraint`, read from the database's typed error, not from `m`.
fn refusal_of(m: &str, constraint: bool) -> ExecError {
    if m.starts_with("release_held:") {
        return ExecError::new(ErrorCode::TenantRefused, m.to_string());
    }
    if m.starts_with("has_referrers:") || m.starts_with("tree_") || constraint {
        return ExecError::new(
            ErrorCode::TenantRefused,
            format!(
                "release_held: this app's release rule removes or reassigns rows that other \
                 rows in this tenant still stand in the way of ({})",
                m.split(':').next().unwrap_or("referrers")
            ),
        );
    }
    if m == ordo_conformance::write::NOT_FOUND {
        return ExecError::new(ErrorCode::NotFound, "this tenant has no such member");
    }
    unavailable(m)
}

fn answer(member: &str, status: &str, rule: &str, effects: Value, door: &str) -> ExecResult {
    ExecResult {
        position: None,
        rows: vec![json!({
            "member": member,
            "status": status,
            "rule": rule,
            "effects": effects,
            "door": door,
        })],
        next_cursor: None,
    }
}
