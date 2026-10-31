//! The guest's half of a share: peek, accept, window and leave
//! (`plan-sharing.md` G3, G4 and G5's leave).
//!
//! [`crate::share_ops`] is the sharer's half: `std.space.share` checks a
//! request and issues a ticket that carries the intent. Nothing exists in the
//! tenant until a guest accepts, and this is that.
//!
//! # Accept, in the order of what cannot be undone
//!
//! 1. **Look, without spending.** The ticket is peeked (`Want` names the
//!    guest as holder, so a ticket that names somebody else is refused and
//!    stays whole), the intent is read from its payload, and the sharer is
//!    checked again against the program as it is now: still a member, still
//!    active, the ops still guard-bearing reads within the sharer's reach.
//!    A program that changed since the share was made refuses here, before
//!    anything is written.
//! 2. **Spend the ticket.** Atomic, so two people racing for one ticket
//!    leave one winner. From here a failure burns the ticket and the answer
//!    says to ask the sharer again.
//! 3. **The seat.** A members row for the guest in the sharer's tenant with
//!    no role, minted or reused. A person who already holds a role is not
//!    given a seat: the share adds a grant to the row they have. The name on
//!    the seat is the one the guest typed, and is the disclosure they
//!    consented to.
//! 4. **The acl rows, as the sharer.** For each row named, the derived
//!    `grant<Entity>Access` runs with the sharer's member id as `$caller`, so
//!    it lands in the sharer's own command chain attributed to the sharer,
//!    and it succeeds only where the sharer administers the row (R-G13). One
//!    that is refused undoes what this accept wrote and refuses the accept.
//! 5. **The grant.** [`register_share`] over the pins as validated now,
//!    recorded with a `shr_` id. A second share to the same person records a
//!    grant for the union of the ops and the later end, then revokes the old
//!    one, so the window never has a gap and never has two ends.
//! 6. **The door.** The tenant is announced so every replica serves the seat.
//! 7. **The consent, in two logs.** `Enrolment { act: "share-accepted" }` in
//!    the deployment's security stream against the guest, mirrored to the
//!    sharer's tenant stream (owner, 2026-09-29: recorded in the sharer's log
//!    and the deployment's security log).
//!
//! # The window
//!
//! [`ShareOps::window`] answers the grant a person holds in a tenant: the
//! consented pins decoded from the registry (never re-pinned from the
//! deployment, so a widened op refuses the window as any grant is refused),
//! and an end of the earlier of the grant's and one session life. A revoked
//! or expired grant answers nothing.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use ordo_api::{BoundParams, OpRegistry};
use ordo_exec::sql_lit;
use ordo_identity::directory::random_principal_suffix;
use ordo_identity::grants::register_share;
use ordo_identity::principal::{GrantId, PrincipalId, SpaceId, Timestamp};
use ordo_identity::{AccessEngine, OpPin};

use crate::share_ops::{validate, ShareOps, ShareRefusal, ShareRow, Timing, GRANT_PREFIX};
use crate::tickets::{Purpose, TicketError, Want};

/// The sentence a guest is told before they consent (`sharing-options.html`
/// B3), because the sharer's tenant owner can reach everything in it.
pub const OWNER_REACH: &str =
    "The owner of this tenant can reach anything in it, and every time they do it is logged.";

/// Writes and takes back the acl rows of a share, as the sharer.
///
/// A trait so the runtime gives it the real executor and a test can give it
/// the fixture's. `caller` is the sharer's member id.
pub trait AclRunner: Send + Sync {
    /// `grant<Entity>Access($<back>: row, $member: member, $level: level)`.
    fn grant<'a>(
        &'a self,
        space: &'a str,
        entity: &'a str,
        row: &'a str,
        member: &'a str,
        level: &'a str,
        caller: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;

    /// `revoke<Entity>Access($<back>: row, $member: member)`.
    fn revoke<'a>(
        &'a self,
        space: &'a str,
        entity: &'a str,
        row: &'a str,
        member: &'a str,
        caller: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;
}

/// What accepting needs beyond what issuing needs.
pub struct AcceptWiring {
    /// The engine the door decides with, read on every call.
    pub engine: Arc<dyn Fn() -> Arc<AccessEngine> + Send + Sync>,
    pub acl: Arc<dyn AclRunner>,
}

/// The derived admin op for a link entity: `grant` or `revoke` over
/// `<entity>Access`, with the first letter of the link's name upper-cased
/// (`boards` derives `grantBoardsAccess`).
pub fn acl_op(verb: &str, entity: &str) -> String {
    let link = format!("{entity}Access");
    let mut chars = link.chars();
    let head: String = chars
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    format!("{verb}{head}{}", chars.as_str())
}

/// [`AclRunner`] over the deployment's executor and its op registry.
pub struct ExecutorAcl {
    pub executor: Arc<dyn ordo_api::OpExecutor>,
    pub registry: Arc<dyn OpRegistry>,
}

impl ExecutorAcl {
    async fn run(
        &self,
        space: &str,
        op: &str,
        entity_param: (&str, &str),
        extra: &[(&str, &str)],
        caller: &str,
    ) -> Result<(), String> {
        let spec = self.registry.get(op).ok_or_else(|| {
            format!(
                "this program has no `{op}`: `{}` is not an acl entity",
                entity_param.0
            )
        })?;
        // The back-reference parameter is the one that is neither `member`
        // nor `level`: `board` for `boards`, whatever the derivation named it.
        let back = spec
            .params
            .iter()
            .map(|p| p.name.as_str())
            .find(|n| *n != "member" && *n != "level")
            .ok_or_else(|| format!("`{op}` names no row parameter"))?
            .to_string();
        let mut m = serde_json::Map::new();
        m.insert(back, serde_json::Value::String(entity_param.1.to_string()));
        for (k, v) in extra {
            m.insert(
                (*k).to_string(),
                serde_json::Value::String((*v).to_string()),
            );
        }
        m.insert(
            "$caller".to_string(),
            serde_json::Value::String(caller.to_string()),
        );
        self.executor
            .execute(&spec, space, BoundParams(m), None, None, None)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

impl AclRunner for ExecutorAcl {
    fn grant<'a>(
        &'a self,
        space: &'a str,
        entity: &'a str,
        row: &'a str,
        member: &'a str,
        level: &'a str,
        caller: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.run(
                space,
                &acl_op("grant", entity),
                (entity, row),
                &[("member", member), ("level", level)],
                caller,
            )
            .await
        })
    }

    fn revoke<'a>(
        &'a self,
        space: &'a str,
        entity: &'a str,
        row: &'a str,
        member: &'a str,
        caller: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.run(
                space,
                &acl_op("revoke", entity),
                (entity, row),
                &[("member", member)],
                caller,
            )
            .await
        })
    }
}

/// The sharer's stated intent, as the ticket carries it.
#[derive(Debug, Clone)]
pub struct Intent {
    pub ops: Vec<String>,
    pub rows: Vec<ShareRow>,
    pub expires_at: u64,
    pub label: String,
}

impl Intent {
    fn parse(payload: Option<&str>) -> Option<Intent> {
        let v: serde_json::Value = serde_json::from_str(payload?).ok()?;
        let ops = v
            .get("ops")?
            .as_array()?
            .iter()
            .filter_map(|o| o.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        let rows = v
            .get("rows")?
            .as_array()?
            .iter()
            .filter_map(|r| {
                Some(ShareRow {
                    entity: r.get("entity")?.as_str()?.to_string(),
                    id: r.get("id")?.as_str()?.to_string(),
                    level: r.get("level")?.as_str()?.to_string(),
                })
            })
            .collect::<Vec<_>>();
        Some(Intent {
            ops,
            rows,
            expires_at: v.get("expiresAt")?.as_u64()?,
            label: v
                .get("label")
                .and_then(|l| l.as_str())
                .unwrap_or_default()
                .to_string(),
        })
    }
}

/// What a guest is shown before they accept. No ticket-holder can learn from
/// it who was named, who issued it or which member ids exist.
#[derive(Debug, Clone)]
pub struct PeekView {
    pub space: String,
    pub name: Option<String>,
    pub label: String,
    pub ops: Vec<String>,
    pub rows: Vec<ShareRow>,
    pub window_ends_at: u64,
    pub ticket_expires_at: u64,
    /// The ticket names a person, and only that person may accept it.
    pub named: bool,
}

/// Why a share was not accepted or a window not opened. `Refused` is the one
/// answer to the caller, with a fixed word for the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShareFailure {
    Refused(&'static str),
    /// A store did not answer, with its typed class.
    Unavailable(ordo_identity::StoreFault),
}

impl From<TicketError> for ShareFailure {
    fn from(e: TicketError) -> ShareFailure {
        match e {
            TicketError::Refused => ShareFailure::Refused("ticket-refused"),
            TicketError::Unavailable(why) => ShareFailure::Unavailable(why),
        }
    }
}

fn unavailable(e: impl crate::auth::store_failure::IntoFault) -> ShareFailure {
    ShareFailure::Unavailable(e.fault())
}

impl From<ordo_api::ExecError> for ShareFailure {
    /// An op's error by its code: `server_busy` stays busy.
    fn from(e: ordo_api::ExecError) -> ShareFailure {
        unavailable(e)
    }
}

/// What accepting made.
#[derive(Debug, Clone)]
pub struct Accepted {
    pub space: String,
    pub name: Option<String>,
    pub seat: String,
    pub grant: String,
    pub expires: u64,
    pub ops: Vec<String>,
    /// The seat was minted by this accept.
    pub seat_new: bool,
}

/// A grant the person holds in a tenant, as the registry consented it.
#[derive(Debug, Clone)]
pub struct Window {
    pub grant: GrantId,
    pub pins: BTreeMap<String, OpPin>,
    pub expires: u64,
}

impl ShareOps {
    fn wiring(&self) -> Result<&AcceptWiring, ShareFailure> {
        self.accept
            .as_ref()
            .ok_or(ShareFailure::Refused("share-not-wired"))
    }

    async fn tenant_name(&self, space: &str) -> Option<String> {
        crate::tenants::lookup(&self.members.control, space)
            .await
            .ok()
            .flatten()
            .map(|t| t.name)
    }

    /// What the ticket offers, without spending it.
    pub async fn peek(&self, ticket: &str) -> Result<PeekView, ShareFailure> {
        let want = Want {
            purpose: Some(Purpose::Share),
            ..Default::default()
        };
        let rec = self.tickets.peek(ticket, &want)?;
        let space = rec
            .space
            .clone()
            .ok_or(ShareFailure::Refused("share-ticket-no-tenant"))?;
        let intent = Intent::parse(rec.payload.as_deref())
            .ok_or(ShareFailure::Refused("share-ticket-no-intent"))?;
        Ok(PeekView {
            name: self.tenant_name(&space).await,
            label: intent.label,
            ops: intent.ops,
            rows: intent.rows,
            window_ends_at: intent.expires_at,
            ticket_expires_at: rec.expires_at,
            named: rec.principal.is_some(),
            space,
        })
    }

    /// Accept a share as `guest`, disclosing `display` on the seat.
    pub async fn accept(
        &self,
        ticket: &str,
        guest: &str,
        display: &str,
    ) -> Result<Accepted, ShareFailure> {
        let wiring = self.wiring()?;
        let now = (self.now)();
        let want = Want {
            purpose: Some(Purpose::Share),
            space: None,
            holder: Some(guest),
        };

        // 1. Look, and check the sharer against the program as it is now.
        let rec = self.tickets.peek(ticket, &want)?;
        let space = rec
            .space
            .clone()
            .ok_or(ShareFailure::Refused("share-ticket-no-tenant"))?;
        let intent = Intent::parse(rec.payload.as_deref())
            .ok_or(ShareFailure::Refused("share-ticket-no-intent"))?;
        if intent.expires_at <= now {
            return Err(ShareFailure::Refused("share-window-ended"));
        }
        // 27ar: a paused tenant takes no new seat. Asked before the ticket
        // is spent, so the same ticket is accepted once the tenant resumes.
        if let Ok(Some(t)) = crate::tenants::lookup(&self.members.control, &space).await {
            if t.state == crate::tenants::State::Paused {
                return Err(ShareFailure::Refused("space-paused"));
            }
        }
        let ispace = SpaceId(space.clone());
        let sharer_principal = rec.issued_by.clone();
        let sharer_member = match self.members.state_of(&space, &sharer_principal).await? {
            Some((role, status)) if status == "active" && !role.is_empty() => self
                .members
                .member_of(&space, &sharer_principal)
                .await?
                .ok_or(ShareFailure::Refused("sharer-gone"))?,
            _ => return Err(ShareFailure::Refused("sharer-gone")),
        };
        let engine = (wiring.engine)();
        let valid = validate(
            &engine,
            &ispace,
            &PrincipalId(sharer_principal.clone()),
            &intent.ops,
            &intent.rows,
            Timing {
                now,
                lifetime_secs: intent.expires_at - now,
                max_secs: None,
            },
        )
        .map_err(|r: ShareRefusal| {
            self.note_refusal(&space, guest, &r.to_string(), now);
            ShareFailure::Refused("share-no-longer-valid")
        })?;
        let existing = self.members.state_of(&space, guest).await?;
        if let Some((role, status)) = &existing {
            if !role.is_empty() && status != "active" {
                return Err(ShareFailure::Refused("member-suspended"));
            }
        }
        let display = display.trim();
        if display.is_empty() || display.chars().count() > 120 {
            return Err(ShareFailure::Refused("display-name"));
        }

        // 2. Spend the ticket. One winner.
        self.tickets.redeem_for(ticket, &want)?;

        // 3. The seat.
        let pool = self.members.pool_of(&space).await?;
        let (seat, seat_new) = self.seat(&pool, &space, guest, display, &existing).await?;

        // 4. The acl rows, as the sharer.
        let mut written: Vec<&ShareRow> = Vec::new();
        for row in &intent.rows {
            // A row the person already holds at any level is left alone. The
            // grant converges on `(row, member)`, so writing `read` over a
            // `write` they hold would take access away.
            match self.acl_row_exists(&pool, &space, row, &seat).await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(why) => {
                    self.note_refusal(&space, guest, &why, now);
                    return Err(ShareFailure::Refused("row-refused"));
                }
            }
            match wiring
                .acl
                .grant(
                    &space,
                    &row.entity,
                    &row.id,
                    &seat,
                    &row.level,
                    &sharer_member,
                )
                .await
            {
                Ok(()) => written.push(row),
                Err(why) => {
                    // A seat this accept minted had no rows before it, so
                    // what was written for it is exactly what to take back.
                    if seat_new {
                        for done in &written {
                            let _ = wiring
                                .acl
                                .revoke(&space, &done.entity, &done.id, &seat, &sharer_member)
                                .await;
                        }
                        let _ = pool
                            .rows(&format!(
                                r#"DELETE FROM "members" WHERE "$space" = {} AND "id" = {}
                                   AND "role" IS NULL"#,
                                sql_lit(&space),
                                sql_lit(&seat)
                            ))
                            .await;
                    }
                    self.note_refusal(&space, guest, &why, now);
                    return Err(ShareFailure::Refused("row-refused"));
                }
            }
        }

        // 5. The grant: the union with any live share to this person, and
        // the later end. Recorded before the old one is revoked.
        let gp = PrincipalId(guest.to_string());
        let held = self
            .grants
            .grants_of(&ispace, &gp)
            .await
            .map_err(unavailable)?;
        let mut pins = valid.pins.clone();
        let mut expires = intent.expires_at;
        let mut old: Vec<GrantId> = Vec::new();
        for r in held
            .iter()
            .filter(|r| r.id.0.starts_with(GRANT_PREFIX) && r.is_live(Timestamp(now)))
        {
            if !old.contains(&r.id) {
                old.push(r.id.clone());
            }
            expires = expires.max(r.expires.0);
            if let Some(before) = ordo_identity::decode_pins(&r.pin) {
                for (op, pin) in before {
                    pins.entry(op).or_insert(pin);
                }
            }
        }
        let grant = GrantId(format!(
            "{GRANT_PREFIX}{}{}",
            random_principal_suffix(),
            random_principal_suffix()
        ));
        let ops: Vec<String> = pins.keys().cloned().collect();
        let new = register_share(
            grant.clone(),
            ispace.clone(),
            gp,
            pins,
            Timestamp(now),
            Timestamp(expires),
        )
        .map_err(|_| ShareFailure::Refused("share-no-longer-valid"))?;
        self.grants.record(&new).await.map_err(unavailable)?;
        for id in &old {
            self.grants
                .revoke(&ispace, id, Timestamp(now))
                .await
                .map_err(unavailable)?;
        }

        // 6. The door, here and on every replica.
        let door = self.members.rebuilt(&space).await;

        // 7. The consent, in the deployment's stream and the sharer's.
        if let Some(stream) = &self.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act: "share-accepted",
                    detail: format!(
                        "seat={seat} seat_new={seat_new} principal={guest} grant={} ops={} \
                         rows={} window_ends={expires} sharer={sharer_principal} door={door}",
                        grant.0,
                        ops.join(","),
                        intent.rows.len(),
                    ),
                    by: Some(guest),
                    tenant: Some(&space),
                    deployment: true,
                    at: now,
                },
            );
        }
        Ok(Accepted {
            name: self.tenant_name(&space).await,
            space,
            seat,
            grant: grant.0,
            expires,
            ops,
            seat_new,
        })
    }

    /// The guest's seat: minted, reactivated, or the row they already hold.
    /// Returns the member id and whether this call minted it.
    async fn seat(
        &self,
        pool: &ordo_exec::Pool,
        space: &str,
        guest: &str,
        display: &str,
        existing: &Option<(String, String)>,
    ) -> Result<(String, bool), ShareFailure> {
        match existing {
            // Holds a role: the share adds a grant to the row they have.
            Some((role, _)) if !role.is_empty() => {
                let id = self
                    .members
                    .member_of(space, guest)
                    .await?
                    .ok_or(ShareFailure::Refused("seat-gone"))?;
                Ok((id, false))
            }
            // A guest seat, possibly suspended by an earlier leave or
            // revocation: reactivated, under the name they now consent to.
            Some(_) => {
                let rows = pool
                    .rows(&format!(
                        r#"UPDATE "members" SET "status" = 'active', "displayName" = {d},
                                                 "rev" = "rev" + 1
                           WHERE "$space" = {s} AND "principal" = {p} AND "kind" = 'principal'
                             AND "role" IS NULL RETURNING "id""#,
                        d = sql_lit(display),
                        s = sql_lit(space),
                        p = sql_lit(guest),
                    ))
                    .await
                    .map_err(unavailable)?;
                let id = rows
                    .first()
                    .and_then(|r| r.first())
                    .and_then(|c| c.clone())
                    .ok_or(ShareFailure::Refused("seat-gone"))?;
                Ok((id, false))
            }
            None => {
                let member = format!("mem_{}", random_principal_suffix());
                let rows = pool
                    .rows(&format!(
                        r#"INSERT INTO "members"
                           ("$space","id","rev","principal","displayName","kind","memberCount",
                            "role","status","joined")
                           SELECT {s}, {m}, 1, {p}, {d}, 'principal', 1, NULL, 'active',
                                  CURRENT_DATE
                           WHERE NOT EXISTS (SELECT 1 FROM "members"
                                             WHERE "$space" = {s} AND "principal" = {p})
                           RETURNING "id""#,
                        s = sql_lit(space),
                        m = sql_lit(&member),
                        p = sql_lit(guest),
                        d = sql_lit(display),
                    ))
                    .await
                    .map_err(unavailable)?;
                if rows.is_empty() {
                    // Another accept for this person landed in between.
                    let id = self
                        .members
                        .member_of(space, guest)
                        .await?
                        .ok_or(ShareFailure::Refused("seat-gone"))?;
                    return Ok((id, false));
                }
                Ok((member, true))
            }
        }
    }

    /// Does `seat` hold any acl row on this row? The link entity is
    /// `<entity>Access` and its back-reference is the entity's name without
    /// a trailing `s`, the rule `acl` derives them by.
    async fn acl_row_exists(
        &self,
        pool: &ordo_exec::Pool,
        space: &str,
        row: &ShareRow,
        seat: &str,
    ) -> Result<bool, String> {
        let back = row.entity.strip_suffix('s').unwrap_or(&row.entity);
        pool.rows(&format!(
            r#"SELECT 1 FROM "{}Access" WHERE "$space" = {} AND "{back}" = {} AND "member" = {}
               LIMIT 1"#,
            row.entity,
            sql_lit(space),
            sql_lit(&row.id),
            sql_lit(seat),
        ))
        .await
        .map(|rows| !rows.is_empty())
        .map_err(|e| format!("`{}` has no acl table here ({e})", row.entity))
    }

    /// The window a person holds in a tenant: the live share grant, its pins
    /// decoded from the registry as they were consented to, and when it ends.
    ///
    /// Nothing is re-pinned from the deployment. A grant carries what its
    /// holder consented to, and an op whose meaning has since widened
    /// refuses the window at the door as it refuses any grant. A revoked, an
    /// expired and a never-made grant are the same answer. A registry that
    /// cannot be read refuses rather than guessing.
    pub async fn window(&self, space: &str, principal: &str) -> Result<Window, ShareFailure> {
        let now = (self.now)();
        let held = self
            .grants
            .grants_of(
                &SpaceId(space.to_string()),
                &PrincipalId(principal.to_string()),
            )
            .await
            .map_err(unavailable)?;
        // One live share grant per person and tenant is the rule; if a race
        // left two, the newest is the window.
        let live: Vec<_> = held
            .iter()
            .filter(|r| r.id.0.starts_with(GRANT_PREFIX) && r.is_live(Timestamp(now)))
            .collect();
        let newest = live
            .iter()
            .max_by(|a, b| (a.granted_at.0, &a.id.0).cmp(&(b.granted_at.0, &b.id.0)))
            .ok_or(ShareFailure::Refused("no-live-share"))?;
        let id = newest.id.clone();
        let mut pins = BTreeMap::new();
        let mut expires = newest.expires.0;
        for r in live.iter().filter(|r| r.id == id) {
            expires = expires.min(r.expires.0);
            let one = ordo_identity::decode_pins(&r.pin)
                .ok_or(ShareFailure::Refused("share-pin-undecodable"))?;
            pins.extend(one);
        }
        Ok(Window {
            grant: id,
            pins,
            expires,
        })
    }

    /// A refused row is the sharer's tenant's business and the stream's: the
    /// guest is told nothing about which row or why.
    fn note_refusal(&self, space: &str, guest: &str, why: &str, at: u64) {
        if let Some(stream) = &self.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act: "share-refused",
                    detail: format!("principal={guest} why={}", why.replace(' ', "_")),
                    by: Some(guest),
                    tenant: Some(space),
                    deployment: true,
                    at,
                },
            );
        }
    }
}
