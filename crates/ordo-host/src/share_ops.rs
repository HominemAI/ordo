//! Sharing between tenants: `std.space.share` (`plan-sharing.md` G2).
//!
//! A share is a window, never administration. The sharer names the ops a
//! guest may call, the rows they may see and how long it lasts. The
//! platform checks the request against the deployed program and the
//! sharer's own reach, and answers a ticket (purpose `Share`) that carries
//! the intent. Nothing is written to the tenant and no grant exists until a
//! guest accepts (G3), so a refused or an unaccepted share leaves no trace
//! but the ticket.
//!
//! # What a share may list
//!
//! Every op must pass all of these, and the first that fails is the answer:
//!
//! 1. **Not a platform op.** A platform op is pinned `unguarded: false`, so
//!    [`register_share`] alone would accept `std.space.callersOf`, and
//!    step-up only keeps a capability token out of it until an app developer
//!    exempts it (`ORDO_STEP_UP_EXEMPT_OPS`). A share is a window.
//! 2. **Deployed.** An op that is not in the program is not shareable.
//! 3. **A read.** A share is read-only. A guest that writes is owed after the
//!    first deployment (checklist 18ab), so an op that assigns any field is
//!    refused.
//! 4. **Not `unguarded`.** [`register_share`]'s own rule
//!    (`guest_exposure_unguarded`, GC064).
//! 5. **With a reachable guard.** At least one entity the op touches carries
//!    a read guard or a `$caller` restriction (GC063). Without one the op
//!    answers every row it can reach, whatever the guest's acl rows say.
//! 6. **Within the sharer's reach.** The sharer must be able to call the op
//!    themselves, by membership, the rule delegation has
//!    (`DelegationBeyondDelegator`). A share carries only the sharer's
//!    authority.
//!
//! A share that lists any `std.<entity>.get` also carries `std.$blob.get`,
//! the file door's own check, so its guest can open the files of the rows
//! it may read ([`carry_files`]). Links and windows share the rule.
//!
//! # Who may
//!
//! Manager and owner, like the other tenant-scoped platform ops, and with
//! presence as the app developer set it for every platform op.
//!
//! # Lifetime
//!
//! `$expires` is a duration from the moment the share is made. The platform
//! sets no ceiling of its own (owner, 2026-09-29: no maximum, "not enforced
//! by us"); a deployment owner may set one with `ORDO_SHARE_MAX_SECS`, and
//! the tenant layer is 24a's. The window closes at that
//! instant whenever the guest accepts. The ticket itself lives no longer
//! than the window and never more than [`crate::tickets::MAX_TTL_SECS`].

use std::collections::BTreeMap;
use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_exec::sql_lit;
use ordo_identity::grants::{register_share, GrantRegistry, ShareError};
use ordo_identity::principal::{GrantId, PrincipalId, SpaceId, Timestamp};
use ordo_identity::{AccessEngine, GuardPin, OpPin};
use ordo_secevent::Recorder;
use serde_json::json;

use crate::member_ops::MemberOps;
use crate::tickets::{Purpose, TicketSpec, Tickets, MAX_TTL_SECS};

pub const SHARE: &str = "std.space.share";
pub const REVOKE_SHARE: &str = "std.space.revokeShare";
/// `std.space.shares`: the tenant's outgoing guest shares, one row per live
/// seat, with its ops and expiry (owner ruling 2026-10-05,
/// `small-readbacks`). The seat id is what `revokeShare` takes.
pub const SHARES: &str = "std.space.shares";

/// Every op of this module, for registration.
pub const OPS: [&str; 3] = [SHARE, REVOKE_SHARE, SHARES];

/// Every share grant's id starts with this. The door consults the grant
/// registry for these and for no other grant (`plan-sharing.md` §3.5).
pub const GRANT_PREFIX: &str = "shr_";

/// The setting a deployment owner may use to cap a share's lifetime.
/// Unset means no ceiling: the platform imposes none of its own.
pub const MAX_SECS_ENV: &str = "ORDO_SHARE_MAX_SECS";

/// How many ops and rows one share may name. A share is a window over a
/// few things; a hundred is far past what a person picks by hand.
const MAX_OPS: usize = 64;
const MAX_ROWS: usize = 100;

/// Read [`MAX_SECS_ENV`]. A value that is not a positive number of seconds
/// refuses the boot rather than being read as the default.
pub fn max_secs_from_env() -> Result<Option<u64>, String> {
    match std::env::var(MAX_SECS_ENV) {
        Err(_) => Ok(None),
        Ok(v) if v.trim().is_empty() => Ok(None),
        Ok(v) => match v.trim().parse::<u64>() {
            Ok(n) if n > 0 => Ok(Some(n)),
            _ => Err(format!(
                "`{v}` is not a positive number of seconds; unset it for no ceiling"
            )),
        },
    }
}

/// Why a share was refused. Each has a stable code, which is what the
/// answer and the tests name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShareRefusal {
    NoOps,
    TooMany(&'static str),
    /// A platform op, named.
    PlatformOp(String),
    /// Not deployed.
    UnknownOp(String),
    /// Assigns a field: a share is read-only.
    WritesData(String),
    /// `guest_exposure_unguarded`, GC064.
    Unguarded(String),
    /// `guest_exposure_unguarded`, GC063: no read guard and no `$caller`
    /// restriction on any entity the op touches.
    NoReachableGuard(String),
    /// The sharer cannot call it.
    BeyondSharer(String),
    BadRows(String),
    BadExpiry(String),
    /// `$to` names nobody who holds a seat here.
    UnknownRecipient,
}

impl ShareRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            ShareRefusal::NoOps | ShareRefusal::TooMany(_) => "share_no_ops",
            ShareRefusal::PlatformOp(_) => "share_platform_op",
            ShareRefusal::UnknownOp(_) => "share_unknown_op",
            ShareRefusal::WritesData(_) => "share_read_only",
            ShareRefusal::Unguarded(_) | ShareRefusal::NoReachableGuard(_) => {
                "guest_exposure_unguarded"
            }
            ShareRefusal::BeyondSharer(_) => "share_beyond_sharer",
            ShareRefusal::BadRows(_) => "share_bad_rows",
            ShareRefusal::BadExpiry(_) => "share_bad_expiry",
            ShareRefusal::UnknownRecipient => "share_unknown_recipient",
        }
    }
}

impl std::fmt::Display for ShareRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = self.code();
        match self {
            ShareRefusal::NoOps => write!(f, "{code}: a share names at least one op"),
            ShareRefusal::TooMany(what) => write!(f, "{code}: a share names too many {what}"),
            ShareRefusal::PlatformOp(op) => write!(
                f,
                "{code}: `{op}` is a platform op, and a share is a window rather than administration"
            ),
            ShareRefusal::UnknownOp(op) => {
                write!(f, "{code}: `{op}` is not an op of this program")
            }
            ShareRefusal::WritesData(op) => write!(
                f,
                "{code}: `{op}` writes, and a share is read-only until a guest may write"
            ),
            ShareRefusal::Unguarded(op) => write!(
                f,
                "{code}: `{op}` is marked unguarded, so sharing it grants every row it can reach \
                 rather than a view of some of them"
            ),
            ShareRefusal::NoReachableGuard(op) => write!(
                f,
                "{code}: `{op}` has no read guard and no `$caller` condition on any entity it \
                 touches, so a guest would see every row it can reach"
            ),
            ShareRefusal::BeyondSharer(op) => write!(
                f,
                "{code}: you cannot call `{op}` yourself, and a share carries only the sharer's \
                 own authority"
            ),
            ShareRefusal::BadRows(why) => write!(f, "{code}: {why}"),
            ShareRefusal::BadExpiry(why) => write!(f, "{code}: {why}"),
            ShareRefusal::UnknownRecipient => write!(
                f,
                "{code}: `$to` names nobody who holds a seat in this tenant; leave it out and \
                 hand the ticket to the person"
            ),
        }
    }
}

/// One row a share opens. Only `read` exists until a guest may write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareRow {
    pub entity: String,
    pub id: String,
    pub level: String,
}

/// Why an op is guard-bearing, as recorded per op (GC065).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Basis {
    /// Entities the op reads through a read guard.
    pub guarded: Vec<String>,
    /// Entities the op restricts to the caller's own rows.
    pub caller_restricted: Vec<String>,
}

/// A share that passed every rule.
#[derive(Debug, Clone)]
pub struct ValidShare {
    /// The pins the grant will carry, decoded from the deployed program.
    pub pins: BTreeMap<String, OpPin>,
    pub ops: Vec<String>,
    pub rows: Vec<ShareRow>,
    /// The instant the window closes.
    pub expires_at: u64,
    /// Per op, what makes it guard-bearing.
    pub basis: BTreeMap<String, Basis>,
}

/// Is `op` one of the platform's, by name or by registration?
fn is_platform(op: &str) -> bool {
    op.starts_with("std.space.")
        || op.starts_with("std.person.")
        || op.starts_with("std.deployment.")
        || crate::platform_ops::is_platform_op(op)
}

fn basis_of(pin: &OpPin) -> Basis {
    let mut b = Basis::default();
    for e in &pin.entities {
        if e.read_guard != GuardPin::Every {
            b.guarded.push(e.entity.clone());
        }
        if e.caller_restricted {
            b.caller_restricted.push(e.entity.clone());
        }
    }
    b
}

/// Parse `$rows`: a JSON array of `{entity, id, level}`.
pub fn parse_rows(text: &str) -> Result<Vec<ShareRow>, ShareRefusal> {
    let bad = |why: &str| ShareRefusal::BadRows(why.to_string());
    let v: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| bad("`$rows` is a JSON array of {entity, id, level}"))?;
    let arr = v
        .as_array()
        .ok_or_else(|| bad("`$rows` is a JSON array of {entity, id, level}"))?;
    if arr.is_empty() {
        return Err(bad("a share names at least one row"));
    }
    if arr.len() > MAX_ROWS {
        return Err(ShareRefusal::TooMany("rows"));
    }
    let name_ok = |s: &str, extra: &[char]| {
        !s.is_empty()
            && s.len() <= 128
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || extra.contains(&c))
    };
    let mut out = Vec::new();
    for r in arr {
        let field = |k: &str| r.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let (Some(entity), Some(id)) = (field("entity"), field("id")) else {
            return Err(bad("every row names an `entity` and an `id`"));
        };
        let level = field("level").unwrap_or_else(|| "read".to_string());
        if !name_ok(&entity, &[]) || !name_ok(&id, &['-', '.', ':']) {
            return Err(bad(
                "an entity or a row id is not one this door can address",
            ));
        }
        if level != "read" {
            return Err(bad(
                "a share opens rows at level `read`; a guest that writes is owed after the \
                 first deployment",
            ));
        }
        out.push(ShareRow { entity, id, level });
    }
    Ok(out)
}

/// Parse a duration like `30d` into seconds. Units: ms s m h d w mo y.
pub fn duration_secs(s: &str) -> Option<u64> {
    let digits = s.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 || digits > 12 {
        return None;
    }
    let n: u64 = s[..digits].parse().ok()?;
    let secs = match &s[digits..] {
        "ms" => n / 1000,
        "s" => n,
        "m" => n.checked_mul(60)?,
        "h" => n.checked_mul(3600)?,
        "d" => n.checked_mul(86_400)?,
        "w" => n.checked_mul(7 * 86_400)?,
        "mo" => n.checked_mul(30 * 86_400)?,
        "y" => n.checked_mul(365 * 86_400)?,
        _ => return None,
    };
    Some(secs)
}

/// When a share is made and how long it may last.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub now: u64,
    /// What the sharer asked for.
    pub lifetime_secs: u64,
    /// The ceiling, when the deployment owner set one.
    pub max_secs: Option<u64>,
}

/// Check a share against the deployed program and the sharer's reach.
///
/// Pure, so the conformance cases that were reserved for a `share to guest:`
/// line (GC063 to GC065) run over it directly.
pub fn validate(
    engine: &AccessEngine,
    space: &SpaceId,
    sharer: &PrincipalId,
    ops: &[String],
    rows: &[ShareRow],
    timing: Timing,
) -> Result<ValidShare, ShareRefusal> {
    let Timing {
        now,
        lifetime_secs,
        max_secs,
    } = timing;
    if ops.is_empty() {
        return Err(ShareRefusal::NoOps);
    }
    if ops.len() > MAX_OPS {
        return Err(ShareRefusal::TooMany("ops"));
    }
    if lifetime_secs == 0 {
        return Err(ShareRefusal::BadExpiry(
            "a share states how long it lasts".to_string(),
        ));
    }
    if let Some(max_secs) = max_secs.filter(|m| lifetime_secs > *m) {
        return Err(ShareRefusal::BadExpiry(format!(
            "this deployment allows a share to last at most {max_secs} seconds \
             (ORDO_SHARE_MAX_SECS={max_secs}). Ask for a shorter share, or raise \
             ORDO_SHARE_MAX_SECS"
        )));
    }
    let mut pins = BTreeMap::new();
    let mut basis = BTreeMap::new();
    let mut listed: Vec<String> = Vec::new();
    for op in ops {
        // The file door's check is carried, never listed: an accept checks
        // the ops the share stored, which hold it when [`carry_files`] added
        // it, and it alone touches no guarded entity.
        if listed.contains(op) || op == ordo_api::blob::BLOB_READ_OP {
            continue;
        }
        if is_platform(op) {
            return Err(ShareRefusal::PlatformOp(op.clone()));
        }
        let Some(pin) = engine.deployed().get(op) else {
            return Err(ShareRefusal::UnknownOp(op.clone()));
        };
        if pin.entities.iter().any(|e| !e.written.is_empty()) {
            return Err(ShareRefusal::WritesData(op.clone()));
        }
        // `register_share` is the registration rule; it is called here, once
        // per op, so its refusal is the one this answers with and the grant
        // G3 records is built by the same function.
        if let Err(ShareError::UnguardedExposure { op }) = register_share(
            GrantId("shr_check".into()),
            space.clone(),
            PrincipalId("guest".into()),
            BTreeMap::from([(op.clone(), pin.clone())]),
            Timestamp(now),
            Timestamp(now + lifetime_secs),
        ) {
            return Err(ShareRefusal::Unguarded(op));
        }
        let b = basis_of(pin);
        if b.guarded.is_empty() && b.caller_restricted.is_empty() {
            return Err(ShareRefusal::NoReachableGuard(op.clone()));
        }
        if !engine.membership_reaches(space, sharer, op) {
            return Err(ShareRefusal::BeyondSharer(op.clone()));
        }
        pins.insert(op.clone(), pin.clone());
        basis.insert(op.clone(), b);
        listed.push(op.clone());
    }
    // A row must belong to an entity some listed op touches, or naming it
    // opens nothing.
    for r in rows {
        let touched = pins
            .values()
            .any(|p| p.entities.iter().any(|e| e.entity == r.entity));
        if !touched {
            return Err(ShareRefusal::BadRows(format!(
                "no shared op touches entity `{}`",
                r.entity
            )));
        }
    }
    carry_files(engine, space, sharer, &mut pins, &mut listed);
    Ok(ValidShare {
        pins,
        ops: listed,
        rows: rows.to_vec(),
        expires_at: now + lifetime_secs,
        basis,
    })
}

/// The file door's own check, carried by a share that reads a row by
/// `std.<entity>.get`.
///
/// `GET /blob/{ref}` checks the pseudo-op `std.$blob.get` and then the row
/// guard over the file's row. A guest has no role, so without the pseudo-op
/// in the grant the file of a row they may read is refused. The rule was the
/// share link's alone until 2026-10-22; a window guest (`std.space.share`,
/// then accept) met the same refusal with no way around it. It lives here so
/// a link, a share and its accept carry the same ops. The row guard still
/// decides which files: only those of rows the guest's acl rows open.
fn carry_files(
    engine: &AccessEngine,
    space: &SpaceId,
    sharer: &PrincipalId,
    pins: &mut BTreeMap<String, OpPin>,
    listed: &mut Vec<String>,
) {
    let blob = ordo_api::blob::BLOB_READ_OP;
    if pins.contains_key(blob)
        || !listed
            .iter()
            .any(|o| o.starts_with("std.") && o.ends_with(".get"))
    {
        return;
    }
    if let Some(pin) = engine.deployed().get(blob) {
        if engine.membership_reaches(space, sharer, blob) {
            pins.insert(blob.to_string(), pin.clone());
            listed.push(blob.to_string());
        }
    }
}

fn refused(r: ShareRefusal) -> ExecError {
    ExecError::new(ErrorCode::TenantRefused, r.to_string())
}

fn text<'a>(bound: &'a BoundParams, name: &str) -> Option<&'a str> {
    bound
        .0
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The state `std.space.share` needs.
pub struct ShareOps {
    pub tickets: Arc<Tickets>,
    pub stream: Option<Arc<Recorder>>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// The ceiling on a share's lifetime, seconds.
    pub max_secs: Option<u64>,
    /// The registry share grants are recorded in and revoked from.
    pub grants: Arc<dyn GrantRegistry>,
    /// The membership commands, for the seat: which database it is in, and
    /// suspending it rebuilds the door and announces to every replica.
    pub members: Arc<MemberOps>,
    /// What accepting a share needs (G3). `None` is a deployment whose
    /// shares can be issued and revoked and not yet accepted.
    pub accept: Option<crate::share_accept::AcceptWiring>,
}

impl ShareOps {
    pub async fn execute(
        &self,
        engine: &AccessEngine,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        match op {
            SHARE => self.share(engine, space, bound, caller),
            REVOKE_SHARE => self.revoke_share(space, bound, caller).await,
            SHARES => self.shares(space).await,
            other => Err(ExecError::internal(format!(
                "`{other}` is not a sharing command"
            ))),
        }
    }

    /// `std.space.share`: check the request, issue the ticket.
    fn share(
        &self,
        engine: &AccessEngine,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let need = |name: &str| {
            text(bound, name)
                .ok_or_else(|| ExecError::internal(format!("`${name}` did not reach the op")))
        };
        let ispace = SpaceId(space.to_string());
        let sharer = engine
            .principal_of_member(&ispace, caller)
            .cloned()
            .unwrap_or_else(|| PrincipalId(caller.to_string()));
        let ops: Vec<String> = need("ops")?
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let rows = parse_rows(need("rows")?).map_err(refused)?;
        let lifetime = duration_secs(need("expires")?).ok_or_else(|| {
            refused(ShareRefusal::BadExpiry(
                "`$expires` is a duration like `30d`".to_string(),
            ))
        })?;
        let now = (self.now)();
        let valid = validate(
            engine,
            &ispace,
            &sharer,
            &ops,
            &rows,
            Timing {
                now,
                lifetime_secs: lifetime,
                max_secs: self.max_secs,
            },
        )
        .map_err(refused)?;
        // A named recipient must already hold a seat here. The ticket is
        // then spendable by that person alone.
        let to = match text(bound, "to") {
            None => None,
            Some(p) => {
                let holds = engine
                    .spaces_of(&PrincipalId(p.to_string()))
                    .iter()
                    .any(|(s, _, _)| *s == ispace);
                if !holds {
                    return Err(refused(ShareRefusal::UnknownRecipient));
                }
                Some(PrincipalId(p.to_string()))
            }
        };
        let label = text(bound, "label").unwrap_or("").to_string();
        let payload = json!({
            "ops": valid.ops,
            "rows": valid.rows.iter().map(|r| json!({
                "entity": r.entity, "id": r.id, "level": r.level,
            })).collect::<Vec<_>>(),
            "expiresAt": valid.expires_at,
            "label": label,
        })
        .to_string();
        let ttl = lifetime.min(MAX_TTL_SECS);
        let issued = self
            .tickets
            .issue_with_payload(
                TicketSpec {
                    purpose: Purpose::Share,
                    principal: to.clone(),
                    display: label,
                    space: Some(space.to_string()),
                    role: None,
                    issued_by: sharer.0.clone(),
                    ttl_secs: ttl,
                },
                Some(payload),
            )
            .map_err(|e| {
                ExecError::of_fault(
                    e.class(),
                    ErrorCode::Internal,
                    format!("the ticket could not be stored: {e}"),
                )
            })?;
        if let Some(stream) = &self.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act: "share-issued",
                    detail: format!(
                        "ops={} rows={} window_ends={} for={} by={}",
                        valid.ops.join(","),
                        valid.rows.len(),
                        valid.expires_at,
                        to.as_ref().map(|p| p.0.as_str()).unwrap_or("-"),
                        sharer.0,
                    ),
                    by: Some(caller),
                    tenant: Some(space),
                    deployment: false,
                    at: now,
                },
            );
        }
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "ticket": issued.ticket,
                "expiresAt": issued.expires_at,
                "windowEndsAt": valid.expires_at,
                "ops": valid.ops,
                "outcome": "issued",
            })],
            next_cursor: None,
        })
    }
}

/// What closing a window did, for the answer and the record.
pub(crate) struct Closed {
    pub(crate) grants: usize,
    pub(crate) acl_rows: usize,
    pub(crate) suspended: bool,
    pub(crate) door: String,
}

/// One seat's live shares, folded.
#[derive(Default)]
struct SeatShare {
    ops: Vec<String>,
    granted: u64,
    expires: u64,
}

/// A store error as an op's: busy is `server_busy`, anything else
/// `space_unavailable` with the store's sentence.
fn unavailable(e: impl crate::auth::store_failure::IntoFault) -> ExecError {
    crate::auth::store_failure::exec_unavailable(e)
}

impl ShareOps {
    /// `std.space.revokeShare`: close a guest's window now.
    ///
    /// In this order, so the door closes first and a failure later leaves
    /// the safe half done: the share grants are revoked in the registry (the
    /// door refuses the next call), the acl rows the shared ops' entities
    /// hold for the seat are deleted, and a seat with no role is suspended
    /// and the door rebuilt. A member who holds a role keeps the membership
    /// and the rows they hold; only the share grant is closed.
    async fn revoke_share(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let seat = text(bound, "seat")
            .ok_or_else(|| ExecError::internal("`$seat` did not reach the op"))?;
        let (principal, guest) = self.seat_of(space, seat).await?;
        let closed = self.close(space, seat, &principal, guest, caller).await?;
        self.record(space, seat, &principal, "share-revoked", &closed, caller);
        Ok(answer(seat, &principal, &closed, "share revoked"))
    }

    /// A guest leaves: the same closing, asked for by the guest themselves.
    /// The route that calls it proves who the caller is; this takes the
    /// principal, finds the seat and closes it. A person with no seat here
    /// is refused as an unknown seat.
    pub async fn leave(&self, space: &str, principal: &str) -> Result<ExecResult, ExecError> {
        let pool = self.members.pool_of(space).await?;
        let rows = pool
            .rows(&format!(
                r#"SELECT "id" FROM "members"
                   WHERE "$space" = {} AND "principal" = {} AND "kind" = 'principal'"#,
                sql_lit(space),
                sql_lit(principal)
            ))
            .await
            .map_err(unavailable)?;
        let seat = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.clone())
            .ok_or_else(|| ExecError::new(ErrorCode::NotFound, "this tenant has no such seat"))?;
        let (principal, guest) = self.seat_of(space, &seat).await?;
        let closed = self.close(space, &seat, &principal, guest, &seat).await?;
        self.record(space, &seat, &principal, "share-left", &closed, &seat);
        Ok(answer(&seat, &principal, &closed, "share left"))
    }

    /// `std.space.shares`: one row per seat that holds a live share grant.
    ///
    /// The tenant's own seats, named by the seat id `revokeShare` takes and
    /// the display name the guest consented to show the sharer. It names
    /// nobody the tenant was not already shown (`plan-sharing.md` §3.3 is
    /// about enumerating people, and this lists the tenant's own grants).
    /// The principal id is not in the answer: the seat is the handle.
    ///
    /// A seat with a role is listed too, marked `guest: false`: a share to a
    /// member adds a grant to the row they hold, and that grant is as much
    /// the tenant's to see and revoke. A grant whose person has no seat here
    /// (the seat was removed under it) is listed with `seat: null`, so the
    /// answer never hides a live grant.
    async fn shares(&self, space: &str) -> Result<ExecResult, ExecError> {
        let ispace = SpaceId(space.to_string());
        let now = (self.now)();
        let live = self
            .grants
            .live_with_prefix(&ispace, GRANT_PREFIX, Timestamp(now))
            .await
            .map_err(unavailable)?;
        // One row per principal: the grant ids, the union of the ops, the
        // latest end. One live share grant per person and tenant is the
        // rule (`share_accept`), and a race that left two is shown as one
        // seat, as the window treats it.
        let mut by_principal: std::collections::BTreeMap<String, SeatShare> =
            std::collections::BTreeMap::new();
        for g in &live {
            let s = by_principal.entry(g.principal.0.clone()).or_default();
            if !s.ops.contains(&g.op) {
                s.ops.push(g.op.clone());
            }
            s.expires = s.expires.max(g.expires.0);
            s.granted = if s.granted == 0 {
                g.granted_at.0
            } else {
                s.granted.min(g.granted_at.0)
            };
        }
        let seats = if by_principal.is_empty() {
            Vec::new()
        } else {
            let pool = self.members.pool_of(space).await?;
            let list = by_principal
                .keys()
                .map(|p| sql_lit(p))
                .collect::<Vec<_>>()
                .join(", ");
            pool.rows(&format!(
                r#"SELECT "principal", "id", "displayName", "role" FROM "members"
                   WHERE "$space" = {} AND "kind" = 'principal' AND "principal" IN ({list})"#,
                sql_lit(space),
            ))
            .await
            .map_err(unavailable)?
        };
        let cell = |r: &Vec<Option<String>>, i: usize| r.get(i).and_then(|c| c.clone());
        let mut rows: Vec<serde_json::Value> = by_principal
            .into_iter()
            .map(|(principal, mut s)| {
                s.ops.sort();
                let seat = seats
                    .iter()
                    .find(|r| cell(r, 0).as_deref() == Some(principal.as_str()));
                json!({
                    "seat": seat.and_then(|r| cell(r, 1)),
                    "displayName": seat.and_then(|r| cell(r, 2)),
                    "guest": seat.is_none_or(|r| cell(r, 3).is_none_or(|v| v.is_empty())),
                    "ops": s.ops,
                    "grantedAt": s.granted,
                    "expiresAt": s.expires,
                })
            })
            .collect();
        rows.sort_by(|a, b| {
            (a["expiresAt"].as_u64(), a["seat"].as_str())
                .cmp(&(b["expiresAt"].as_u64(), b["seat"].as_str()))
        });
        Ok(ExecResult {
            position: None,
            rows,
            next_cursor: None,
        })
    }

    /// The principal a seat belongs to and whether it holds no role.
    pub(crate) async fn seat_of(
        &self,
        space: &str,
        seat: &str,
    ) -> Result<(String, bool), ExecError> {
        let not_found = || ExecError::new(ErrorCode::NotFound, "this tenant has no such seat");
        if !ordo_api::well_formed_space(seat) {
            return Err(not_found());
        }
        let pool = self.members.pool_of(space).await?;
        let rows = pool
            .rows(&format!(
                r#"SELECT "principal", "role" FROM "members"
                   WHERE "$space" = {} AND "id" = {} AND "kind" = 'principal'"#,
                sql_lit(space),
                sql_lit(seat)
            ))
            .await
            .map_err(unavailable)?;
        let r = rows.first().ok_or_else(not_found)?;
        let cell = |i: usize| r.get(i).and_then(|c| c.clone()).unwrap_or_default();
        Ok((cell(0), cell(1).is_empty()))
    }

    pub(crate) async fn close(
        &self,
        space: &str,
        seat: &str,
        principal: &str,
        guest: bool,
        caller: &str,
    ) -> Result<Closed, ExecError> {
        let ispace = SpaceId(space.to_string());
        let now = (self.now)();
        let held = self
            .grants
            .grants_of(&ispace, &PrincipalId(principal.to_string()))
            .await
            .map_err(unavailable)?;
        // The live share grants, and the entities their ops touched: those
        // are the tables that can hold acl rows this share wrote.
        let mut ids: Vec<GrantId> = Vec::new();
        let mut entities: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for rec in held
            .iter()
            .filter(|r| r.id.0.starts_with(GRANT_PREFIX) && r.is_live(Timestamp(now)))
        {
            if !ids.contains(&rec.id) {
                ids.push(rec.id.clone());
            }
            if let Some(pins) = ordo_identity::decode_pins(&rec.pin) {
                for pin in pins.values() {
                    entities.extend(pin.entities.iter().map(|e| e.entity.clone()));
                }
            }
        }
        for id in &ids {
            self.grants
                .revoke(&ispace, id, Timestamp(now))
                .await
                .map_err(unavailable)?;
        }
        let pool = self.members.pool_of(space).await?;
        let mut acl_rows = 0;
        // Only a seat with no role has its acl rows taken back. A member may
        // hold rows in those tables by ordinary means, and until a share
        // records which rows it wrote (G3) there is no telling them apart.
        for entity in entities
            .iter()
            .filter(|_| guest)
            .filter(|e| e.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        {
            let table = format!("{entity}Access");
            let exists = pool
                .rows(&format!(
                    r#"SELECT to_regclass({}) IS NOT NULL"#,
                    sql_lit(&format!("\"{table}\""))
                ))
                .await
                .map_err(unavailable)?;
            if !matches!(
                exists.first().and_then(|r| r[0].as_deref()),
                Some("t") | Some("true")
            ) {
                continue;
            }
            let gone = pool
                .rows(&format!(
                    r#"DELETE FROM "{table}" WHERE "$space" = {} AND "member" = {} RETURNING "id""#,
                    sql_lit(space),
                    sql_lit(seat)
                ))
                .await
                .map_err(unavailable)?;
            acl_rows += gone.len();
        }
        let mut door = String::from("membership kept");
        let mut suspended = false;
        if guest {
            let mut m = serde_json::Map::new();
            m.insert("member".into(), json!(seat));
            let out = self
                .members
                .execute(crate::member_ops::SUSPEND, space, &BoundParams(m), caller)
                .await?;
            suspended = true;
            door = out
                .rows
                .first()
                .and_then(|r| r["door"].as_str())
                .unwrap_or("")
                .to_string();
        }
        // Last, once the door refuses the window: a feed that re-reads on this
        // signal before the seat is suspended would still be let in.
        // Tell every open feed on this tenant that closing the window may
        // have changed what it reads. The deletes above are raw SQL, not a
        // write op, so they signalled nothing; the per-note take back is a
        // write op and signals its entity at commit. This sends the same
        // `ordo.invalidate` signal for each entity the share touched and its
        // `<Entity>Access` table. A feed on the window then re-reads, is
        // refused at the closed door and ends with an error frame.
        for entity in &entities {
            for name in [entity.clone(), format!("{entity}Access")] {
                pool.rows(&format!(
                    "SELECT pg_notify({}, {})",
                    sql_lit(ordo_exec::jobs::INVALIDATE_CHANNEL),
                    sql_lit(&name)
                ))
                .await
                .map_err(unavailable)?;
            }
        }
        Ok(Closed {
            grants: ids.len(),
            acl_rows,
            suspended,
            door,
        })
    }

    /// The act, in the tenant's stream and against the person in the
    /// deployment's (`plan-sharing.md` §3.4).
    fn record(&self, space: &str, seat: &str, principal: &str, act: &str, c: &Closed, by: &str) {
        if let Some(stream) = &self.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act: if act == "share-left" {
                        "share-left"
                    } else {
                        "share-revoked"
                    },
                    detail: format!(
                        "seat={seat} principal={principal} grants={} acl_rows={} suspended={} by={by}",
                        c.grants, c.acl_rows, c.suspended
                    ),
                    by: Some(by),
                    tenant: Some(space),
                    deployment: true,
                    at: (self.now)(),
                },
            );
        }
    }
}

fn answer(seat: &str, principal: &str, c: &Closed, outcome: &str) -> ExecResult {
    ExecResult {
        position: None,
        rows: vec![json!({
            "seat": seat,
            "principal": principal,
            "grants": c.grants,
            "aclRows": c.acl_rows,
            "suspended": c.suspended,
            "outcome": outcome,
            "door": c.door,
        })],
        next_cursor: None,
    }
}
