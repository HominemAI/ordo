//! Tenant invitations: `std.space.invite`, `std.space.invitations` and
//! `std.space.revokeInvitation` (checklist 24e).
//!
//! An invitation is a ticket of purpose `Invitation` for one tenant
//! (`plan-enrolment.md` E2). Sign-in redeems it (E4, `Admission::on_arrival`)
//! and admits the person at the role it names. Until these ops nothing
//! issued one, so a tenant whose admission is invitation-only admitted
//! nobody.
//!
//! # Who may
//!
//! Tenant owner and manager, like the other tenant-scoped platform ops, and
//! with presence as the app developer set it for every platform op
//! (`ORDO_STEP_UP_EXEMPT_OPS`). A manager may not invite above their own
//! role, so an invitation cannot be used to become an owner by proxy.
//!
//! # The secret
//!
//! The answer to `std.space.invite` carries the ticket once. The app
//! delivers it. Ordo does not send it. Only the hash is stored, so the
//! listing names an invitation by an id and never shows the secret.
//!
//! # Role
//!
//! A role is one of owner, manager, editor, viewer. When the inviter names
//! none, the ticket names none and the tenant's own default applies when it
//! is redeemed: viewer, unless the app developer set another through
//! `std.space.setAdmission`.
//!
//! # Lifetime
//!
//! `$expires` is a duration such as `7d`, stated per invitation. When it is
//! absent the ticket default applies ([`crate::tickets::DEFAULT_TTL_SECS`]).
//! This module adds no maximum. The ticket store still clamps every ticket
//! to [`crate::tickets::MAX_TTL_SECS`], a fixed value the settings audit
//! (24c) owes a setting; the answer's `expiresAt` is the instant that
//! applies, so a caller sees a clamp.

use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_identity::principal::{PrincipalId, SpaceId};
use ordo_identity::AccessEngine;
use ordo_secevent::Recorder;
use serde_json::json;

use crate::member_ops::ROLES;
use crate::share_ops::duration_secs;
use crate::tickets::{id_of, Purpose, TicketSpec, Tickets};

pub const INVITE: &str = "std.space.invite";
pub const INVITATIONS: &str = "std.space.invitations";
pub const REVOKE_INVITATION: &str = "std.space.revokeInvitation";

/// Every op of this module, for registration.
pub const OPS: [&str; 3] = [INVITE, INVITATIONS, REVOKE_INVITATION];

fn refused(why: impl Into<String>) -> ExecError {
    ExecError::new(ErrorCode::TenantRefused, why.into())
}

fn text<'a>(bound: &'a BoundParams, name: &str) -> Option<&'a str> {
    bound
        .0
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn level(role: &str) -> Option<u8> {
    match role {
        "viewer" => Some(0),
        "editor" => Some(1),
        "manager" => Some(2),
        "owner" => Some(3),
        _ => None,
    }
}

/// The state the invitation ops need.
pub struct InviteOps {
    pub tickets: Arc<Tickets>,
    pub stream: Option<Arc<Recorder>>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl InviteOps {
    pub fn execute(
        &self,
        engine: &AccessEngine,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        match op {
            INVITE => self.invite(engine, space, bound, caller),
            INVITATIONS => self.invitations(space),
            REVOKE_INVITATION => self.revoke(space, bound, caller),
            other => Err(ExecError::internal(format!(
                "`{other}` is not an invitation command"
            ))),
        }
    }

    fn invite(
        &self,
        engine: &AccessEngine,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let ispace = SpaceId(space.to_string());
        let inviter = engine
            .principal_of_member(&ispace, caller)
            .cloned()
            .unwrap_or_else(|| PrincipalId(caller.to_string()));

        let role = match text(bound, "role") {
            None => None,
            Some(r) if ROLES.contains(&r) => Some(r.to_string()),
            Some(_) => return Err(refused("a role is one of owner, manager, editor or viewer")),
        };
        // An invitation carries at most the authority of whoever issues it.
        if let Some(r) = &role {
            let mine = engine
                .spaces_of(&inviter)
                .into_iter()
                .find(|(s, _, _)| *s == ispace)
                .and_then(|(_, role, _)| role)
                .map(|role| role.level());
            if mine.is_none_or(|m| level(r).unwrap_or(u8::MAX) > m) {
                return Err(refused(
                    "an invitation cannot name a role above the role of the member who issues it",
                ));
            }
        }
        let ttl_secs = match text(bound, "expires") {
            None => 0,
            Some(d) => match duration_secs(d) {
                Some(n) if n > 0 => n,
                _ => return Err(refused("`$expires` is a duration like `7d`")),
            },
        };
        let to = match text(bound, "to") {
            None => None,
            Some(p) if ordo_api::well_formed_space(p) => Some(PrincipalId(p.to_string())),
            Some(_) => return Err(refused("`$to` is not a well-formed principal id")),
        };
        let label = text(bound, "label").unwrap_or("").to_string();
        let now = (self.now)();
        let issued = self
            .tickets
            .issue(TicketSpec {
                purpose: Purpose::Invitation,
                principal: to.clone(),
                display: label,
                space: Some(space.to_string()),
                role: role.clone(),
                issued_by: inviter.0.clone(),
                ttl_secs,
            })
            .map_err(|e| {
                ExecError::of_fault(
                    e.class(),
                    ErrorCode::Internal,
                    format!("the ticket could not be stored: {e}"),
                )
            })?;
        self.record(
            "invitation-issued",
            format!(
                "role={} for={} expires={} by={}",
                role.as_deref().unwrap_or("default"),
                to.as_ref().map(|p| p.0.as_str()).unwrap_or("-"),
                issued.expires_at,
                inviter.0,
            ),
            space,
            caller,
            now,
        );
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "ticket": issued.ticket,
                "expiresAt": issued.expires_at,
                "role": role,
                "outcome": "issued",
            })],
            next_cursor: None,
        })
    }

    /// The invitations that can still be redeemed, by id, then the ones the
    /// invitee declined, until each would have expired (owner ruling
    /// 2026-10-05: the inviter can see an invitation was declined). Never
    /// the secret.
    fn invitations(&self, space: &str) -> Result<ExecResult, ExecError> {
        let open = self
            .tickets
            .list_open(space, Purpose::Invitation)
            .map_err(ExecError::from)?;
        let declined = self
            .tickets
            .list_declined(space, Purpose::Invitation)
            .map_err(ExecError::from)?;
        let row = |r: &crate::tickets::TicketRecord, declined_at: Option<u64>| {
            json!({
                "id": id_of(r),
                "role": r.role,
                "for": r.principal.as_ref().map(|p| p.0.as_str()),
                "label": r.display,
                "issuedBy": r.issued_by,
                "issuedAt": r.issued_at,
                "expiresAt": r.expires_at,
                "status": if declined_at.is_some() { "declined" } else { "open" },
                "declinedAt": declined_at,
            })
        };
        let rows = open
            .iter()
            .map(|r| row(r, None))
            .chain(declined.iter().map(|(r, at)| row(r, Some(*at))))
            .collect();
        Ok(ExecResult {
            position: None,
            rows,
            next_cursor: None,
        })
    }

    fn revoke(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let id =
            text(bound, "id").ok_or_else(|| ExecError::internal("`$id` did not reach the op"))?;
        let n = self
            .tickets
            .revoke_open(space, Purpose::Invitation, id)
            .map_err(ExecError::from)?;
        // One answer for an id that never existed, one already used and one
        // that belongs to another tenant.
        if n == 0 {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "this tenant has no such open invitation",
            ));
        }
        let now = (self.now)();
        self.record(
            "invitation-revoked",
            format!("id={id} by={caller}"),
            space,
            caller,
            now,
        );
        Ok(ExecResult {
            position: None,
            rows: vec![json!({ "id": id, "outcome": "revoked" })],
            next_cursor: None,
        })
    }

    /// The tenant's own log, and no other: an invitation is an act on a
    /// membership, which is the tenant's business (`enrolment::Act`).
    fn record(&self, act: &str, detail: String, space: &str, caller: &str, at: u64) {
        if let Some(stream) = &self.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act,
                    detail,
                    by: Some(caller),
                    tenant: Some(space),
                    deployment: false,
                    at,
                },
            );
        }
    }
}
