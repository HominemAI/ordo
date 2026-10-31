//! The deployment owner's commands on a person's credential and on a tenant's
//! owner (`plan-passkey-recovery.md` R5):
//! `std.identity.recover`, `std.identity.recoveryTicket`,
//! `std.identity.cancelRecovery` and `std.space.succeedOwner`; and a tenant
//! owner's part (R6): `std.space.recoverMember` and
//! `std.space.setRecoveryPolicy`.
//!
//! # Who may
//!
//! **The deployment owner, and nobody inside a tenant** (owner, 2026-09-29:
//! *"Yes the deployment owner."*). The ops are owner-only and step-up gated
//! like the rest, and they answer only in the deployment's own space, which a
//! tenant's owner or manager is not a member of. So a tenant's manager cannot
//! recover a tenant's owner: the door refuses them before this code runs, and
//! this code would refuse the tenant's space if it did. That is the rule the
//! plan gives its reason for: a manager who could recover an owner would make
//! *manager* a role that becomes *owner* by reporting a lost key.
//!
//! # A recovery through the door
//!
//! [`RECOVER`] opens a case for a person, naming the class of evidence the
//! deployment owner relied on and an opaque reference into their own ticketing
//! system. No free text enters the stream. The case matures after the
//! deployment's delay (a day by default), during which any passkey the person
//! still holds can cancel it. [`RECOVERY_TICKET`] collects the ticket once the
//! case has matured, and the person spends it by registering a new passkey
//! (`/auth/recover/complete`). Everything a recovery does follows: every
//! session ends, the new passkey is quarantined from step-up, the passkeys they
//! had can contest.
//!
//! The record is at two scopes. The deployment's stream names the deployment
//! owner who acted. Each tenant the person belongs to is told, in its own
//! stream, that the deployment acted, as *deployment-owner* and not by name,
//! and the person's own first sign-in afterwards carries the case until they
//! acknowledge it.
//!
//! The evidence class is **recorded, not checked**. The platform cannot tell
//! whether a deployment owner verified somebody out of band. Recording the
//! class makes the claim specific and attributable, so an auditor can ask for
//! the evidence behind it. Each case says so (`checked=no`).
//!
//! # When the owner is gone for good
//!
//! A person who has left, or died, will not redeem a ticket. The act needed is
//! a membership act, and it is [`SUCCEED_OWNER`]: the deployment owner makes an
//! existing member of the tenant an owner. It touches no credential and ends
//! no session. It is separate from recovery so the record says which of the two
//! happened, and it has the same evidence classes and the same delay: the first
//! call opens a case, and a second call with `case` after the delay carries it
//! out.
//!
//! # The tenant's managers are told, and may object
//!
//! Owner, 2026-10-24 (`plan-passkey-recovery.md` ask 8, managers only). A
//! succession of a tenant, and a recovery through the door of a person who
//! owns a tenant, concern that tenant's owner. Each **manager** of the tenant
//! is told at their next sign-in or token refresh (`owner_cases` beside
//! `recoveries`), and any of them may object with [`OBJECT_TO_CASE`] in the
//! tenant during the delay. An objection **pauses** the case: it does not
//! mature, yields no ticket and cannot be carried out until the deployment
//! owner resolves it with [`RESOLVE_OBJECTION`], resume or cancel, with a
//! reason from a short fixed list. [`OPEN_CASES`] lists the open cases, with
//! each objection and its resolution, for the console.
//!
//! # Closing a contest window early
//!
//! A completed recovery leaves a contest window (72 hours by default) in
//! which a passkey it retired may undo it, and no new case opens for the
//! person. [`CLOSE_CONTEST`] ends the window now, with a reason from
//! [`crate::recovery::CLOSE_CONTEST_REASONS`] (`plan-passkey-recovery.md`
//! §4.2, audit F-3 2026-10-24). The retired passkeys can no longer contest,
//! and a new case may be opened at once. A passkey the closed case added
//! still cannot cancel that new case until the time the window was given has
//! passed. Recorded in the deployment's stream and each tenant's.
//!
//! # Completing a succession ends the departed owner's delegations
//!
//! Owner, 2026-10-24. The deployment owner names who has left when the case
//! opens (`departed`, required, each an owner of the tenant then, never the
//! successor). Only those named are departed: a living co-owner never loses
//! anything because someone else's succession completed (coordinator,
//! 2026-10-24). Completing the succession ends each named one's live
//! delegations in the tenant and the grant to the app each
//! one's consent minted (`crate::consent::Consents`), unless the completing
//! call says `endGrants: false` or the deployment set
//! `ORDO_SUCCESSION_ENDS_GRANTS=off`. The record names each one ended; the
//! new owner can grant again.

use std::sync::{Arc, OnceLock};

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_identity::directory::DirectoryStore;
use ordo_identity::principal::PrincipalId;
use serde_json::json;

use crate::member_ops::MemberOps;
use crate::recovery::{CompleteError, Open, OpenError, Path, Recovery};

pub const RECOVER: &str = "std.identity.recover";
pub const RECOVERY_TICKET: &str = "std.identity.recoveryTicket";
pub const CANCEL_RECOVERY: &str = "std.identity.cancelRecovery";
pub const SUCCEED_OWNER: &str = "std.space.succeedOwner";
/// A tenant owner recovers a member the tenant created (R6).
pub const RECOVER_MEMBER: &str = "std.space.recoverMember";
/// A tenant chooses whether a recovered person is held (R6).
pub const SET_RECOVERY_POLICY: &str = "std.space.setRecoveryPolicy";
/// Another deployment owner approves a case or a succession (R7).
pub const APPROVE_RECOVERY: &str = "std.identity.approveRecovery";
/// Another owner of the tenant approves a member's recovery (R7).
pub const APPROVE_MEMBER_RECOVERY: &str = "std.space.approveMemberRecovery";
/// A tenant's manager objects to a case about its owner, which pauses it.
pub const OBJECT_TO_CASE: &str = "std.space.objectToCase";
/// The deployment owner resolves the objections to a case: resume or cancel.
pub const RESOLVE_OBJECTION: &str = "std.identity.resolveObjection";
/// The open recoveries through the door and successions, with objections.
pub const OPEN_CASES: &str = "std.identity.openCases";
/// The deployment owner closes a completed recovery's contest window early.
pub const CLOSE_CONTEST: &str = "std.identity.closeContest";

/// Every op of this module, for registration and for the owner-only list.
pub const OPS: [&str; 12] = [
    RECOVER,
    RECOVERY_TICKET,
    CANCEL_RECOVERY,
    SUCCEED_OWNER,
    RECOVER_MEMBER,
    SET_RECOVERY_POLICY,
    APPROVE_RECOVERY,
    APPROVE_MEMBER_RECOVERY,
    OBJECT_TO_CASE,
    RESOLVE_OBJECTION,
    OPEN_CASES,
    CLOSE_CONTEST,
];

/// The ops that answer only in the deployment's own space. The others answer
/// in a tenant, for that tenant's owner.
fn deployment_scoped(op: &str) -> bool {
    !matches!(
        op,
        RECOVER_MEMBER | SET_RECOVERY_POLICY | APPROVE_MEMBER_RECOVERY | OBJECT_TO_CASE
    )
}

/// The one sentence for every case in which a tenant may not recover a
/// member. It does not say why: not that the person belongs to another
/// tenant, and not what the person is, so that a refusal tells an
/// administrator nothing about who else a person belongs to.
const NOT_YOURS: &str = "a tenant may recover a member only when it created them, they belong \
                         nowhere else, and they are not an owner. Refer this person to the \
                         deployment owner";

/// The classes of evidence a case may name. A closed list: no free text
/// enters the stream.
/// What a manager is told when the setting does not reach them. One sentence
/// for every reason (the deployment has not allowed it, the tenant turned it
/// off, the member is a manager or an owner, the member is themselves), so a
/// manager learns nothing about a peer's role from the refusal.
const MANAGER_NOT_ALLOWED: &str = "a manager may recover a member only when the app developer \
                                   has allowed it and this tenant has not turned it off, and \
                                   never a manager or an owner. Ask a tenant owner";

pub const EVIDENCE: &[&str] = &[
    "vouched-by-members",
    "verified-out-of-band",
    "contract-holder",
    "host-break-glass",
];

/// The reasons a case may be cancelled for.
pub const CANCEL_REASONS: &[&str] = &["opened-in-error", "not-verified", "person-withdrew"];

/// Recovery, filled in once it exists. The platform ops are built before the
/// authentication state that owns recovery, so they hold this and the runtime
/// sets it when recovery is wired.
pub type RecoveryHandle = Arc<OnceLock<Arc<Recovery>>>;

pub struct IdentityOps {
    pub recovery: RecoveryHandle,
    /// So a case names a person who exists.
    pub directory: Option<Arc<dyn DirectoryStore>>,
    /// Owner succession is a membership act.
    pub members: Arc<MemberOps>,
    /// The deployment's own space, the only one the deployment owner's ops
    /// answer in.
    pub home: String,
    /// Who created whom, for the tenant's recovery of a member (R6). `None`
    /// where the deployment has no credentials, and the op then says so.
    pub managed: Option<crate::credential_ops::ManagedStore>,
    /// What each tenant has chosen about recovery (R6).
    pub tenants: Option<crate::recovery_hold::TenantRecovery>,
    /// The consent records, so completing a succession can end the departed
    /// owners' delegations and the grants to apps that came with them.
    /// `None` where the deployment has none; a completion that asked for
    /// them to end then says so in its record.
    pub consents: Option<crate::consent::Consents>,
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

/// An opaque reference into the deployment owner's own ticketing system: short
/// and from a narrow alphabet, so nothing a caller sent can be text.
fn reference_of(bound: &BoundParams) -> Result<&str, ExecError> {
    let r = text(bound, "reference").unwrap_or("");
    if r.len() > 120
        || !r
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:/-".contains(c))
    {
        return Err(refused(
            "a reference is at most 120 characters from A-Z a-z 0-9 . _ : / -",
        ));
    }
    Ok(r)
}

fn evidence_of(bound: &BoundParams) -> Result<&str, ExecError> {
    let e = required(bound, "evidence")?;
    if EVIDENCE.contains(&e) {
        Ok(e)
    } else {
        Err(refused(&format!(
            "evidence is one of: {}",
            EVIDENCE.join(", ")
        )))
    }
}

fn approval_error(e: CompleteError) -> ExecError {
    match e {
        CompleteError::Refused(_) => refused(
            "that case cannot be approved by you: it does not exist or is not waiting, you are \
             the person it is about, or you have approved it already",
        ),
        CompleteError::Unavailable(_, failure) => {
            failure.exec_error("the approval could not be recorded")
        }
    }
}

/// What a case paused by an objection answers, naming the op that resolves
/// it.
fn paused() -> ExecError {
    refused(
        "that case is paused by a manager's objection. The deployment owner resolves it with \
         std.identity.resolveObjection (resume or cancel, with a reason)",
    )
}

fn disabled() -> ExecError {
    ExecError::new(
        ErrorCode::TenantRefused,
        "this deployment has switched that recovery path off",
    )
}

fn unwired() -> ExecError {
    ExecError::new(
        ErrorCode::TenantRefused,
        "this deployment has no credential recovery configured (it needs passkey \
         authentication, ORDO_AUTH_METHODS)",
    )
}

impl IdentityOps {
    fn recovery(&self) -> Result<&Arc<Recovery>, ExecError> {
        self.recovery.get().ok_or_else(unwired)
    }

    /// Run an op by name.
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        // Who is acting, as a person: the door hands an op the member id, and
        // recovery names and counts people, so it resolves the id to the
        // principal once, here. An id that names nobody is used as it is.
        let resolved = self
            .members
            .principal_of_member(space, caller)
            .await?
            .unwrap_or_else(|| caller.to_string());
        let caller = resolved.as_str();
        // A tenant's own ops: the owner of the tenant, in the tenant.
        match op {
            RECOVER_MEMBER => return self.recover_member(space, bound, caller).await,
            SET_RECOVERY_POLICY => return self.set_policy(space, bound, caller).await,
            APPROVE_MEMBER_RECOVERY => return self.approve_member(space, bound, caller).await,
            OBJECT_TO_CASE => return self.object(space, bound, caller).await,
            _ => {}
        }
        // Deployment-scoped, like `std.deployment.apply`: it answers in the
        // deployment's own space and nowhere else. A tenant's owner is not a
        // member of that space, so the door has already refused them, and
        // this is the second lock on the same door.
        debug_assert!(deployment_scoped(op));
        if space != self.home {
            return Err(refused(
                "this op answers only in the deployment's own space: credentials belong to \
                 the deployment, and no tenant administers them",
            ));
        }
        match op {
            RECOVER => self.recover(bound, caller).await,
            RECOVERY_TICKET => self.ticket(bound).await,
            CANCEL_RECOVERY => self.cancel(bound, caller).await,
            SUCCEED_OWNER => self.succeed(bound, caller).await,
            APPROVE_RECOVERY => self.approve(bound, caller).await,
            RESOLVE_OBJECTION => self.resolve(bound, caller).await,
            OPEN_CASES => self.open_cases(bound).await,
            CLOSE_CONTEST => self.close_contest(bound, caller).await,
            other => Err(ExecError::internal(format!(
                "`{other}` is not an identity command"
            ))),
        }
    }

    fn known(&self, principal: &str) -> Result<(), ExecError> {
        if !ordo_api::well_formed_space(principal) {
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
        Ok(())
    }

    async fn recover(&self, bound: &BoundParams, caller: &str) -> Result<ExecResult, ExecError> {
        let principal = required(bound, "principal")?;
        let evidence = evidence_of(bound)?;
        let reference = reference_of(bound)?;
        self.known(principal)?;
        let recovery = self.recovery()?.clone();
        let (principal_s, evidence_s, reference_s, caller_s) = (
            principal.to_string(),
            evidence.to_string(),
            reference.to_string(),
            caller.to_string(),
        );
        let opened = tokio::task::spawn_blocking(move || {
            recovery.open(&Open {
                principal: &principal_s,
                path: Path::Operator,
                opened_by: &caller_s,
                authority: "deployment-owner",
                evidence: &evidence_s,
                reference: &reference_s,
                space: "",
                extra_delay_secs: 0,
                approvals: 0,
            })
        })
        .await
        .map_err(|_| ExecError::internal("the recovery task failed"))?
        .map_err(|e| match e {
            OpenError::Live => ExecError::new(
                ErrorCode::Duplicate,
                "that person already has a recovery in progress: open, matured, or inside \
                 its contest window",
            ),
            OpenError::Disabled => disabled(),
            OpenError::OutsideProvider => refused(crate::recovery::OUTSIDE_PROVIDER_SENTENCE),
            OpenError::Unavailable(_) => ExecError::new(
                ErrorCode::SpaceUnavailable,
                "the recovery could not be opened",
            ),
        })?;
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "case": opened.case,
                "principal": principal,
                "state": if opened.ticket.is_some() { "matured" } else { "pending" },
                "maturesAt": opened.matures_at,
                "approvalsRequired": opened.approvals_required,
                "ticket": opened.ticket,
                "ticketExpiresAt": opened.ticket_expires_at,
                "outcome": "opened",
            })],
            next_cursor: None,
        })
    }

    async fn ticket(&self, bound: &BoundParams) -> Result<ExecResult, ExecError> {
        let case = required(bound, "case")?.to_string();
        let recovery = self.recovery()?.clone();
        let c2 = case.clone();
        let issued = tokio::task::spawn_blocking(move || recovery.issue_ticket(&c2))
            .await
            .map_err(|_| ExecError::internal("the recovery task failed"))?
            .map_err(|e| match e {
                CompleteError::Refused("case-paused") => paused(),
                CompleteError::Refused(_) => refused(
                    "that case has not matured, does not exist, or already yielded its ticket",
                ),
                CompleteError::Unavailable(_, failure) => {
                    failure.exec_error("the ticket could not be issued")
                }
            })?;
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "case": case,
                "ticket": issued.0,
                "ticketExpiresAt": issued.1,
                "outcome": "ticket issued",
            })],
            next_cursor: None,
        })
    }

    async fn cancel(&self, bound: &BoundParams, caller: &str) -> Result<ExecResult, ExecError> {
        let case = required(bound, "case")?.to_string();
        let why = text(bound, "reason").unwrap_or("opened-in-error");
        if !CANCEL_REASONS.contains(&why) {
            return Err(refused(&format!(
                "a reason is one of: {}",
                CANCEL_REASONS.join(", ")
            )));
        }
        let recovery = self.recovery()?.clone();
        let (c2, why2, caller2) = (case.clone(), why.to_string(), caller.to_string());
        let moved = tokio::task::spawn_blocking(move || {
            recovery.cancel(&c2, ordo_secevent::Subject::member(&caller2), &why2)
        })
        .await
        .map_err(|_| ExecError::internal("the recovery task failed"))?
        .map_err(|_| ExecError::new(ErrorCode::SpaceUnavailable, "the case could not be read"))?;
        if !moved {
            return Err(refused(
                "that case does not exist or has already completed. A completed recovery is \
                 contested by the person's own passkey",
            ));
        }
        Ok(ExecResult {
            position: None,
            rows: vec![json!({"case": case, "outcome": "cancelled"})],
            next_cursor: None,
        })
    }

    /// A tenant's owner recovers a member the tenant created (R6, path C).
    ///
    /// Only for a person the tenant created, who belongs nowhere else, and
    /// who is not an owner: the tenant then has the power over the credential
    /// (owner, 2026-09-29: *"the tenant should have the power for it and
    /// should be logged appropriately"*) and no other tenant is reached by it.
    /// **Every refusal is the one sentence [`NOT_YOURS`]**, so an
    /// administrator learns nothing about which case it was, and the reason
    /// class goes to the stream of the tenant whose administrator was
    /// refused. An owner is restored by the deployment owner and by nobody
    /// inside the tenant, another owner included.
    ///
    /// Without `case` it opens a case and, when the tenant's delay is zero,
    /// returns the ticket at once. With `case`, after the delay, it collects
    /// the ticket.
    /// May `caller` act on a recovery of `principal` in `space`? A tenant
    /// owner may, for any member below owner (the caller checks the target
    /// is not an owner). A manager may only when the app developer has set
    /// `ORDO_RECOVERY_MANAGERS`, the tenant has not turned it off, and the
    /// member is below manager. Nobody recovers themselves. Returns the
    /// reason word for the stream when not.
    async fn administrator_may(
        &self,
        space: &str,
        caller: &str,
        principal: &str,
        target_role: &str,
        chosen: &crate::recovery_hold::TenantSettings,
    ) -> Result<Result<(), &'static str>, ExecError> {
        if caller == principal {
            return Ok(Err("target-is-self"));
        }
        let role = self
            .members
            .state_of(space, caller)
            .await?
            .map(|(role, _)| role)
            .unwrap_or_default();
        Ok(match role.as_str() {
            "owner" => Ok(()),
            "manager" => {
                let allowed =
                    self.recovery()?.policy().managers_may_recover && chosen.managers_enabled;
                if !allowed {
                    Err("managers-not-allowed")
                } else if matches!(target_role, "owner" | "manager") {
                    Err("target-not-below-manager")
                } else {
                    Ok(())
                }
            }
            _ => Err("caller-not-an-administrator"),
        })
    }

    async fn tenant_settings(
        &self,
        space: &str,
    ) -> Result<crate::recovery_hold::TenantSettings, ExecError> {
        match &self.tenants {
            Some(t) => t.settings(space).await.map_err(ExecError::from),
            None => Ok(crate::recovery_hold::TenantSettings {
                hold: false,
                delay_secs: 0,
                approvals: 1,
                path_enabled: true,
                codes_enabled: true,
                managers_enabled: true,
            }),
        }
    }

    async fn recover_member(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let member = required(bound, "member")?;
        let recovery = self.recovery()?.clone();
        let Some(managed) = &self.managed else {
            return Err(unwired());
        };
        let refuse = |reason: &str| {
            recovery.record_split(
                "refused",
                (
                    ordo_secevent::Subject::member(caller),
                    format!(
                        "path=tenant reason={reason} member={member} by={caller} space={space}"
                    ),
                ),
                (
                    ordo_secevent::Subject::member(caller),
                    format!("path=tenant reason={reason} member={member} by={caller}"),
                ),
                vec![space.to_string()],
            );
            refused(NOT_YOURS)
        };
        let principal =
            match crate::credential_ops::authorise_managed(&self.members, managed, space, member)
                .await
            {
                Ok(p) => p,
                Err(e) if e.code == ErrorCode::TenantRefused => {
                    return Err(refuse("not-managed-here"))
                }
                Err(e) => return Err(e),
            };
        let role = self
            .members
            .state_of(space, &principal)
            .await?
            .map(|(role, _)| role)
            .unwrap_or_default();
        if role == "owner" {
            return Err(refuse("target-is-owner"));
        }
        // What the tenant has chosen for its own members: it may have switched
        // its administrators' recovery off, and it may ask for more delay and
        // more approvals than the deployment does. The deployment's floor is
        // what `Recovery::open` applies when it is the stricter.
        let chosen = self.tenant_settings(space).await?;
        if let Err(reason) = self
            .administrator_may(space, caller, &principal, &role, &chosen)
            .await?
        {
            recovery.record_split(
                "refused",
                (
                    ordo_secevent::Subject::member(caller),
                    format!(
                        "path=tenant reason={reason} member={member} by={caller} space={space}"
                    ),
                ),
                (
                    ordo_secevent::Subject::member(caller),
                    format!("path=tenant reason={reason} member={member} by={caller}"),
                ),
                vec![space.to_string()],
            );
            return Err(refused(MANAGER_NOT_ALLOWED));
        }
        if !chosen.path_enabled {
            return Err(refuse("path-disabled-by-tenant"));
        }
        if let Some(case_id) = text(bound, "case") {
            let case = recovery
                .case(case_id)
                .map_err(|_| {
                    ExecError::new(ErrorCode::SpaceUnavailable, "the case could not be read")
                })?
                .filter(|c| c.principal == principal && c.space == space && c.path == Path::Tenant)
                .ok_or_else(|| refused("that case is not one of this tenant's"))?;
            let (r2, id) = (recovery.clone(), case.id.clone());
            let issued = tokio::task::spawn_blocking(move || r2.issue_ticket(&id))
                .await
                .map_err(|_| ExecError::internal("the recovery task failed"))?
                .map_err(|e| match e {
                    CompleteError::Refused(_) => {
                        refused("that case has not matured, or already yielded its ticket")
                    }
                    CompleteError::Unavailable(_, failure) => {
                        failure.exec_error("the ticket could not be issued")
                    }
                })?;
            return Ok(ExecResult {
                position: None,
                rows: vec![json!({
                    "case": case.id,
                    "member": member,
                    "principal": principal,
                    "ticket": issued.0,
                    "ticketExpiresAt": issued.1,
                    "outcome": "ticket issued",
                })],
                next_cursor: None,
            });
        }
        let (p, c, sp) = (principal.clone(), caller.to_string(), space.to_string());
        let opened = tokio::task::spawn_blocking(move || {
            recovery.open(&Open {
                principal: &p,
                path: Path::Tenant,
                opened_by: &c,
                authority: "tenant-administrator",
                evidence: "tenant-administrator",
                reference: "",
                space: &sp,
                extra_delay_secs: chosen.delay_secs,
                approvals: chosen.approvals,
            })
        })
        .await
        .map_err(|_| ExecError::internal("the recovery task failed"))?
        .map_err(|e| match e {
            OpenError::Live => ExecError::new(
                ErrorCode::Duplicate,
                "that person already has a recovery in progress",
            ),
            OpenError::Disabled => disabled(),
            OpenError::OutsideProvider => refused(crate::recovery::OUTSIDE_PROVIDER_SENTENCE),
            OpenError::Unavailable(_) => ExecError::new(
                ErrorCode::SpaceUnavailable,
                "the recovery could not be opened",
            ),
        })?;
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "case": opened.case,
                "member": member,
                "principal": principal,
                "state": if opened.ticket.is_some() { "matured" } else { "pending" },
                "maturesAt": opened.matures_at,
                "approvalsRequired": opened.approvals_required,
                "ticket": opened.ticket,
                "ticketExpiresAt": opened.ticket_expires_at,
                "outcome": "opened",
            })],
            next_cursor: None,
        })
    }

    /// A tenant's owner chooses how recovery treats the people it created:
    /// whether a recovered person is held (R6), and (R7) a longer delay, more
    /// approvals, no recovery by the tenant's administrators, no recovery
    /// codes. **A tenant can only tighten.** A delay or an approvals count
    /// below the deployment's is refused, since a tenant loosening a floor
    /// would weaken a credential other tenants rely on.
    async fn set_policy(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let Some(tenants) = &self.tenants else {
            return Err(unwired());
        };
        let recovery = self.recovery()?.clone();
        let policy = recovery.policy().clone();
        let flag = |name: &str| bound.0.get(name).and_then(|v| v.as_bool());
        let number = |name: &str| bound.0.get(name).and_then(|v| v.as_u64());
        let patch = crate::recovery_hold::Patch {
            hold: flag("hold"),
            delay_secs: number("delaySecs"),
            approvals: number("approvals").map(|n| n.min(u64::from(u32::MAX)) as u32),
            path_enabled: flag("pathEnabled"),
            codes_enabled: flag("codesEnabled"),
            managers_enabled: flag("managersMayRecover"),
        };
        if patch == crate::recovery_hold::Patch::default() {
            return Err(refused(
                "name at least one setting: hold, delaySecs, approvals, pathEnabled, codesEnabled or \
                 managersMayRecover",
            ));
        }
        if let Some(d) = patch.delay_secs {
            if d < policy.tenant_delay_secs {
                return Err(refused(&format!(
                    "this deployment sets a floor of {} seconds on a tenant's recovery delay, and \
                     a tenant may only raise it",
                    policy.tenant_delay_secs
                )));
            }
        }
        if let Some(a) = patch.approvals {
            if a < policy.tenant_approvals {
                return Err(refused(&format!(
                    "this deployment sets a floor of {} approvals on a tenant's recovery of a \
                     member, and a tenant may only raise it",
                    policy.tenant_approvals
                )));
            }
            if a > crate::recovery::MAX_APPROVALS {
                return Err(refused(&format!(
                    "approvals are between 1 and {}",
                    crate::recovery::MAX_APPROVALS
                )));
            }
        }
        if patch.managers_enabled == Some(true) && !policy.managers_may_recover {
            return Err(refused(
                "this deployment does not let a tenant's managers recover members, and a \
                 tenant may only turn that off for itself, never on",
            ));
        }
        let at = self.members.at();
        let now = tenants
            .update(space, &patch, caller, at)
            .await
            .map_err(ExecError::from)?;
        recovery.record_split(
            "policy-set",
            (
                ordo_secevent::Subject::member(caller),
                format!(
                    "space={space} hold={} delay-secs={} approvals={} path-enabled={} \
                     codes-enabled={} managers-enabled={} by={caller}",
                    now.hold,
                    now.delay_secs,
                    now.approvals,
                    now.path_enabled,
                    now.codes_enabled,
                    now.managers_enabled
                ),
            ),
            (
                ordo_secevent::Subject::member(caller),
                format!(
                    "hold={} delay-secs={} approvals={} path-enabled={} codes-enabled={} \
                     managers-enabled={} by={caller}",
                    now.hold,
                    now.delay_secs,
                    now.approvals,
                    now.path_enabled,
                    now.codes_enabled,
                    now.managers_enabled
                ),
            ),
            vec![space.to_string()],
        );
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "space": space,
                "hold": now.hold,
                "delaySecs": now.delay_secs,
                "approvals": now.approvals,
                "pathEnabled": now.path_enabled,
                "codesEnabled": now.codes_enabled,
                "managersMayRecover": policy.managers_may_recover && now.managers_enabled,
                "outcome": "set",
            })],
            next_cursor: None,
        })
    }

    /// Another deployment owner approves a case opened through the door, or a
    /// succession. Neither the person being recovered nor somebody who has
    /// approved already may, and the opener is the first approval. Answers how
    /// many approvals the case has and how many it needs.
    async fn approve(&self, bound: &BoundParams, caller: &str) -> Result<ExecResult, ExecError> {
        let case = required(bound, "case")?.to_string();
        let recovery = self.recovery()?.clone();
        let (c2, by) = (case.clone(), caller.to_string());
        let (have, need) = tokio::task::spawn_blocking(move || {
            recovery.approve(&c2, &by, &[Path::Operator, Path::Succession])
        })
        .await
        .map_err(|_| ExecError::internal("the recovery task failed"))?
        .map_err(approval_error)?;
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "case": case,
                "approvals": have,
                "approvalsRequired": need,
                "outcome": if have >= need { "approved" } else { "waiting for more approvals" },
            })],
            next_cursor: None,
        })
    }

    /// Another owner of the tenant approves a recovery of a member it created.
    /// The same rules as [`IdentityOps::approve`], and the case must be this
    /// tenant's, for this member.
    async fn approve_member(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let member = required(bound, "member")?;
        let case_id = required(bound, "case")?.to_string();
        let recovery = self.recovery()?.clone();
        let Some(managed) = &self.managed else {
            return Err(unwired());
        };
        let principal =
            match crate::credential_ops::authorise_managed(&self.members, managed, space, member)
                .await
            {
                Ok(p) => p,
                Err(e) if e.code == ErrorCode::TenantRefused => return Err(refused(NOT_YOURS)),
                Err(e) => return Err(e),
            };
        let target_role = self
            .members
            .state_of(space, &principal)
            .await?
            .map(|(role, _)| role)
            .unwrap_or_default();
        let chosen = self.tenant_settings(space).await?;
        if self
            .administrator_may(space, caller, &principal, &target_role, &chosen)
            .await?
            .is_err()
        {
            return Err(refused(MANAGER_NOT_ALLOWED));
        }
        let case = recovery
            .case(&case_id)
            .map_err(|_| ExecError::new(ErrorCode::SpaceUnavailable, "the case could not be read"))?
            .filter(|c| c.principal == principal && c.space == space && c.path == Path::Tenant)
            .ok_or_else(|| refused("that case is not one of this tenant's for that member"))?;
        let (id, by) = (case.id.clone(), caller.to_string());
        let (have, need) =
            tokio::task::spawn_blocking(move || recovery.approve(&id, &by, &[Path::Tenant]))
                .await
                .map_err(|_| ExecError::internal("the recovery task failed"))?
                .map_err(approval_error)?;
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "case": case.id,
                "member": member,
                "approvals": have,
                "approvalsRequired": need,
                "outcome": if have >= need { "approved" } else { "waiting for more approvals" },
            })],
            next_cursor: None,
        })
    }

    /// Owner succession. Without `case` it opens one and answers when it may
    /// be carried out. With `case`, once the delay has passed, it makes the
    /// named member an owner.
    async fn succeed(&self, bound: &BoundParams, caller: &str) -> Result<ExecResult, ExecError> {
        let tenant = required(bound, "tenant")?;
        if !ordo_api::well_formed_space(tenant) {
            return Err(refused("that is not a tenant id this door can address"));
        }
        if tenant == self.home {
            return Err(refused(
                "the deployment's own space has its owner set on the server side, not here",
            ));
        }
        let recovery = self.recovery()?.clone();
        match text(bound, "case") {
            None => {
                let principal = required(bound, "principal")?;
                let evidence = evidence_of(bound)?;
                let reference = reference_of(bound)?;
                self.known(principal)?;
                let Some((role, status)) = self.members.state_of(tenant, principal).await? else {
                    return Err(refused(
                        "that person is not a member of that tenant. Owner succession makes an \
                         existing member the owner and admits nobody",
                    ));
                };
                if status != "active" {
                    return Err(refused("that member is not active in that tenant"));
                }
                if role == "owner" {
                    return Err(refused("that member is already an owner of that tenant"));
                }
                // Who has left, named by the deployment owner: only they lose
                // their delegations when the succession completes.
                let departed = departed_of(bound, principal)?;
                let owners = self.members.owners_of(tenant).await?;
                if let Some(not) = departed.iter().find(|d| !owners.contains(d)) {
                    return Err(refused(&format!(
                        "`{not}` is not an owner of that tenant. `departed` names the owners who \
                         have left, each an owner of the tenant now"
                    )));
                }
                let (p, e, r, c, t) = (
                    principal.to_string(),
                    evidence.to_string(),
                    reference.to_string(),
                    caller.to_string(),
                    tenant.to_string(),
                );
                let d2 = departed.clone();
                let opened = tokio::task::spawn_blocking(move || {
                    recovery.open_succession(
                        &Open {
                            principal: &p,
                            path: Path::Succession,
                            opened_by: &c,
                            authority: "deployment-owner",
                            evidence: &e,
                            reference: &r,
                            space: &t,
                            extra_delay_secs: 0,
                            approvals: 0,
                        },
                        &d2,
                    )
                })
                .await
                .map_err(|_| ExecError::internal("the recovery task failed"))?
                .map_err(|e| match e {
                    OpenError::Live => ExecError::new(
                        ErrorCode::Duplicate,
                        "that person already has a case in progress",
                    ),
                    OpenError::Disabled => disabled(),
                    OpenError::OutsideProvider => {
                        refused(crate::recovery::OUTSIDE_PROVIDER_SENTENCE)
                    }
                    OpenError::Unavailable(_) => {
                        ExecError::new(ErrorCode::SpaceUnavailable, "the case could not be opened")
                    }
                })?;
                Ok(ExecResult {
                    position: None,
                    rows: vec![json!({
                        "case": opened.case,
                        "tenant": tenant,
                        "principal": principal,
                        "maturesAt": opened.matures_at,
                        "departed": departed,
                        "outcome": "opened",
                    })],
                    next_cursor: None,
                })
            }
            Some(case_id) => {
                // Whether the departed owners' delegations end: the call's
                // word, or the deployment's default (on).
                let end_grants = bound
                    .0
                    .get("endGrants")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(recovery.policy().succession_ends_grants);
                let (r2, id2) = (recovery.clone(), case_id.to_string());
                let case = tokio::task::spawn_blocking(move || r2.mature_succession(&id2))
                    .await
                    .map_err(|_| ExecError::internal("the recovery task failed"))?
                    .map_err(|e| match e {
                        CompleteError::Refused("case-paused") => paused(),
                        CompleteError::Refused(_) => refused(
                            "that succession has not matured, does not exist, or is not one",
                        ),
                        CompleteError::Unavailable(_, failure) => {
                            failure.exec_error("the case could not be read")
                        }
                    })?;
                if case.space != tenant {
                    return Err(refused("that case is about another tenant"));
                }
                let Some((member, was)) = self.members.make_owner(tenant, &case.principal).await?
                else {
                    return Err(refused(
                        "that member is no longer an active member of the tenant",
                    ));
                };
                let (r3, c3) = (recovery.clone(), case.clone());
                let closed = tokio::task::spawn_blocking(move || r3.close_succession(&c3))
                    .await
                    .map_err(|_| ExecError::internal("the recovery task failed"))?
                    .map_err(|_| {
                        ExecError::new(ErrorCode::SpaceUnavailable, "the case could not be closed")
                    })?;
                let mut ended = crate::recovery::Ended {
                    on: end_grants,
                    ..Default::default()
                };
                if closed {
                    if end_grants {
                        self.end_departed(tenant, &case, &mut ended).await;
                    }
                    recovery.record_succession(&case, caller, &ended);
                }
                let door = self.members.rebuilt(tenant).await;
                Ok(ExecResult {
                    position: None,
                    rows: vec![json!({
                        "case": case.id,
                        "tenant": tenant,
                        "principal": case.principal,
                        "member": member,
                        "from": was,
                        "role": "owner",
                        "departed": departed_list(&case.departed),
                        "endsGrants": end_grants,
                        "grantsEnded": ended.grants,
                        "delegationsEnded": ended.delegations,
                        "notEnded": ended.failed,
                        "outcome": if closed { "succeeded" } else { "already carried out" },
                        "door": door,
                    })],
                    next_cursor: None,
                })
            }
        }
    }

    /// End each departed owner's live delegations in the tenant and the
    /// grant to the app each one's consent minted. Every one is named in
    /// `ended`, and any that could not be ended is named in `ended.failed`.
    async fn end_departed(
        &self,
        tenant: &str,
        case: &crate::recovery::Case,
        ended: &mut crate::recovery::Ended,
    ) {
        let Some(consents) = &self.consents else {
            ended.failed.push("consent-records-absent".to_string());
            return;
        };
        let now = self.members.at();
        for owner in departed_list(&case.departed) {
            if owner == case.principal {
                continue;
            }
            let (gone, failed) = consents.end_all_of(tenant, &owner, now).await;
            for d in gone {
                for g in &d.grants {
                    ended.grants.push(format!("{g}@{}", d.agent));
                }
                ended.delegations.push(format!("{}@{}", d.id, d.agent));
            }
            ended.failed.extend(failed);
        }
    }

    /// A manager of this tenant objects to a case about its owner, which
    /// pauses the case until the deployment owner resolves it. Nobody else
    /// in the tenant may: an owner, an editor or a viewer is refused (plan
    /// ask 8: one editor must not be able to stall a legitimate case).
    async fn object(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let case_id = required(bound, "case")?.to_string();
        let recovery = self.recovery()?.clone();
        let state = self.members.state_of(space, caller).await?;
        if !matches!(
            state.as_ref().map(|(r, s)| (r.as_str(), s.as_str())),
            Some(("manager", "active"))
        ) {
            recovery.record_split(
                "refused",
                (
                    ordo_secevent::Subject::member(caller),
                    format!(
                        "op=object case={case_id} reason=not-a-manager space={space} by={caller}"
                    ),
                ),
                (
                    ordo_secevent::Subject::member(caller),
                    format!("op=object case={case_id} reason=not-a-manager by={caller}"),
                ),
                vec![space.to_string()],
            );
            return Err(refused(
                "only an active manager of this tenant may object to a case about its owner",
            ));
        }
        let (c2, by, sp) = (case_id.clone(), caller.to_string(), space.to_string());
        let case = tokio::task::spawn_blocking(move || recovery.object(&c2, &by, &sp))
            .await
            .map_err(|_| ExecError::internal("the recovery task failed"))?
            .map_err(|e| match e {
                CompleteError::Refused(_) => refused(
                    "that is not an open case about this tenant's owner, it is about you, or you \
                     have objected to it already",
                ),
                CompleteError::Unavailable(_, failure) => {
                    failure.exec_error("the objection could not be recorded")
                }
            })?;
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "case": case.id,
                "tenant": space,
                "path": case.path.as_str(),
                "state": "paused",
                "outcome": "objected",
            })],
            next_cursor: None,
        })
    }

    /// The deployment owner resolves the objections to a case: `resume` lets
    /// it go on, `cancel` ends it, each with a reason from its list.
    async fn resolve(&self, bound: &BoundParams, caller: &str) -> Result<ExecResult, ExecError> {
        let case_id = required(bound, "case")?.to_string();
        let decision = required(bound, "decision")?;
        let Some(resolution) = crate::recovery::Resolution::parse(decision) else {
            return Err(refused("a decision is `resume` or `cancel`"));
        };
        let reason = required(bound, "reason")?;
        if !resolution.reasons().contains(&reason) {
            return Err(refused(&format!(
                "a reason to {} is one of: {}",
                resolution.as_str(),
                resolution.reasons().join(", ")
            )));
        }
        let recovery = self.recovery()?.clone();
        let (c2, r2, by) = (case_id.clone(), reason.to_string(), caller.to_string());
        let (n, state) =
            tokio::task::spawn_blocking(move || recovery.resolve(&c2, resolution, &r2, &by))
                .await
                .map_err(|_| ExecError::internal("the recovery task failed"))?
                .map_err(|e| match e {
                    CompleteError::Refused(_) => refused(
                        "that case does not exist, is not open, or has no objection waiting",
                    ),
                    CompleteError::Unavailable(_, failure) => {
                        failure.exec_error("the decision could not be recorded")
                    }
                })?;
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "case": case_id,
                "decision": resolution.as_str(),
                "reason": reason,
                "objections": n,
                "state": state.as_str(),
                "outcome": "resolved",
            })],
            next_cursor: None,
        })
    }

    /// Close a completed recovery's contest window now, with a reason from
    /// the fixed list.
    async fn close_contest(
        &self,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let case_id = required(bound, "case")?.to_string();
        let reason = required(bound, "reason")?;
        if !crate::recovery::CLOSE_CONTEST_REASONS.contains(&reason) {
            return Err(refused(&format!(
                "a reason to close a contest is one of: {}",
                crate::recovery::CLOSE_CONTEST_REASONS.join(", ")
            )));
        }
        let Some(directory) = self.directory.clone() else {
            return Err(unwired());
        };
        let recovery = self.recovery()?.clone();
        let (c2, r2, by) = (case_id.clone(), reason.to_string(), caller.to_string());
        let closed = tokio::task::spawn_blocking(move || {
            recovery.close_contest(directory.as_ref(), &c2, &r2, &by)
        })
        .await
        .map_err(|_| ExecError::internal("the recovery task failed"))?
        .map_err(|e| match e {
            CompleteError::Refused(_) => refused(
                "that case does not exist or is not a completed recovery in its contest window",
            ),
            CompleteError::Unavailable(_, failure) => {
                failure.exec_error("the contest could not be closed")
            }
        })?;
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "case": closed.case,
                "principal": closed.principal,
                "reason": reason,
                "retired": closed.retired.iter().map(|a| a.0.clone()).collect::<Vec<_>>(),
                "state": "closed",
                "outcome": "contest-closed",
            })],
            next_cursor: None,
        })
    }

    /// The open recoveries through the door and successions, oldest first,
    /// each with when it matures, its approvals, and every objection and its
    /// resolution. `tenant` narrows to the successions of one tenant.
    async fn open_cases(&self, bound: &BoundParams) -> Result<ExecResult, ExecError> {
        let only = text(bound, "tenant").map(str::to_string);
        let recovery = self.recovery()?.clone();
        let rows = tokio::task::spawn_blocking(move || {
            let now = recovery.clock();
            let mut rows = Vec::new();
            for (case, objections) in recovery.open_cases()? {
                if only.as_deref().is_some_and(|t| t != case.space) {
                    continue;
                }
                let (have, need) = recovery.progress(&case.id)?;
                let paused = objections.iter().any(|o| o.resolved_at == 0);
                rows.push(json!({
                    "case": case.id,
                    "path": case.path.as_str(),
                    "tenant": case.space,
                    "principal": case.principal,
                    "evidence": case.evidence,
                    "reference": case.reference,
                    "openedAt": case.opened_at,
                    "maturesAt": case.matures_at,
                    "state": case.state.as_str(),
                    "mature": case.matures_at <= now && !paused,
                    "paused": paused,
                    "approvals": have,
                    "approvalsRequired": need,
                    "departed": departed_list(&case.departed),
                    "objections": objections.iter().map(|o| json!({
                        "objector": o.objector,
                        "tenant": o.space,
                        "at": o.at,
                        "resolvedAt": if o.resolved_at == 0 { None } else { Some(o.resolved_at) },
                        "resolution": if o.resolution.is_empty() { None } else { Some(o.resolution.clone()) },
                        "reason": if o.reason.is_empty() { None } else { Some(o.reason.clone()) },
                    })).collect::<Vec<_>>(),
                }));
            }
            Ok::<_, ordo_identity::StoreFault>(rows)
        })
        .await
        .map_err(|_| ExecError::internal("the recovery task failed"))?
        .map_err(|_| ExecError::new(ErrorCode::SpaceUnavailable, "the cases could not be read"))?;
        Ok(ExecResult {
            position: None,
            rows,
            next_cursor: None,
        })
    }
}

/// The most owners one succession may name as departed.
const MAX_DEPARTED: usize = 10;

/// `departed`: the owners who have left, a list of principal ids. Required
/// when a succession opens, at least one, distinct, never the successor.
fn departed_of(bound: &BoundParams, successor: &str) -> Result<Vec<String>, ExecError> {
    let missing = || {
        refused(
            "name who has left: `departed` is a list of 1 to 10 principal ids, each an owner of \
             the tenant, never the successor",
        )
    };
    let list = bound
        .0
        .get("departed")
        .and_then(|v| v.as_array())
        .ok_or_else(missing)?;
    let mut out: Vec<String> = Vec::new();
    for v in list {
        let id = v.as_str().map(str::trim).unwrap_or_default();
        if !ordo_api::well_formed_space(id) {
            return Err(missing());
        }
        if id == successor {
            return Err(refused("the successor cannot also be named as departed"));
        }
        if !out.iter().any(|o| o == id) {
            out.push(id.to_string());
        }
    }
    if out.is_empty() || out.len() > MAX_DEPARTED {
        return Err(missing());
    }
    Ok(out)
}

/// A case's departed owners as a list.
fn departed_list(departed: &str) -> Vec<String> {
    departed
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
