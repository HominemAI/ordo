//! Pausing a tenant (checklist 27ar, `plan-tenant-pause.md`, owner rulings of
//! 2026-10-05).
//!
//! # What the owner ruled
//!
//! - A tenant can be paused and resumed. A paused tenant refuses its ops and
//!   keeps its data.
//! - **Pause is a deployment operation** (ask 1). Exposing it to a tenant is
//!   the app's choice, through the door's own role rules (ask 4): by default
//!   only the deployment owner pauses and resumes, and the app developer
//!   names the tenant roles that may ([`PausePolicy`]).
//! - **An op the app marks `nopause` keeps answering** (asks 2 and 7):
//!   `op billing nopause = ...`.
//! - The layer outside a paused tenant is the deployment's own space (ask 3).
//!   Nothing new is built for it: sign-in is deployment-wide, and the
//!   person's tenant list says which of their tenants are paused.
//! - A notice for members is the app's to build (ask 6). The reason recorded
//!   with a pause is a word from a closed list, never free text.
//!
//! # The four ops
//!
//! | Op | Bound to | Who, by default |
//! |---|---|---|
//! | `std.deployment.pauseTenant($tenant, $note?)` | the deployment's own space | its owner |
//! | `std.deployment.resumeTenant($tenant)` | the deployment's own space | its owner; lifts either kind of pause |
//! | `std.space.pause($note?)` | the tenant | nobody, until the app developer names roles |
//! | `std.space.resume` | the tenant | nobody, until the app developer names roles; lifts only a pause made from inside |
//!
//! # The hold, and how every replica learns of it
//!
//! The registry row (`$spaces`) is the record: `state = 'paused'`, with when,
//! by which layer and why. Every replica holds the paused tenants in memory
//! ([`crate::spaces::Spaces::paused_record`]) so the gate is a map read. A
//! pause or resume updates the replica that took it at once and announces on
//! `ordo.members`, and every other replica reloads from the registry when it
//! hears that. The `tenants.pause.refresh` worker reloads on the access
//! refresh period, which is the ceiling.
//!
//! # What answers on a paused tenant
//!
//! The gate ([`gate`]) runs in the platform executor, after the door's access
//! check, so a caller with no membership learns nothing about the pause.
//! It refuses with `space_paused` (423) unless the op is one of these:
//!
//! - an app op the program marks `nopause`;
//! - a platform op [`answers_while_paused`] lists: resume, export, delete,
//!   the chain proof, `callersOf`, the attestation read-backs, the member
//!   commands that take access away or change a role, the lists of shares,
//!   links and invitations and their revocations, the admission read-back,
//!   and the person's own tenant list.
//!
//! Everything else is refused, derived `std.*` ops included: no new member
//! is admitted, nothing is shared, no bucket or attestation destination
//! changes, nobody leaves, and the blob door neither stores nor reads.

use std::sync::{Arc, OnceLock};

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_exec::Pool;
use ordo_identity::principal::Role;
use ordo_secevent::Recorder;
use serde_json::json;

use crate::platform_ops::PLATFORM_VERSION;
use crate::spaces::Spaces;
use crate::tenants::{self, Paused, Resumed, PAUSED_BY_DEPLOYMENT, PAUSED_BY_TENANT, PAUSE_NOTES};

/// The tenant pauses itself. Granted to no role until the app developer
/// names some ([`PAUSE_ROLES_ENV`]).
pub const PAUSE: &str = "std.space.pause";
/// The tenant lifts its own pause. Granted as [`RESUME_ROLES_ENV`] says.
pub const RESUME: &str = "std.space.resume";
/// The deployment owner pauses a tenant, from the deployment's own space.
pub const PAUSE_TENANT: &str = "std.deployment.pauseTenant";
/// The deployment owner resumes a tenant, from the deployment's own space.
pub const RESUME_TENANT: &str = "std.deployment.resumeTenant";

pub const OPS: [&str; 4] = [PAUSE, RESUME, PAUSE_TENANT, RESUME_TENANT];

/// The ops a tenant role may be granted. The other two are the deployment
/// owner's and stay so.
pub fn tenant_scoped(name: &str) -> bool {
    name == PAUSE || name == RESUME
}

/// The deployment owner's two, answered only in the deployment's own space.
pub fn deployment_scoped(name: &str) -> bool {
    name == PAUSE_TENANT || name == RESUME_TENANT
}

/// The four specs.
pub fn specs() -> Vec<OpSpec> {
    let text = |name: &str, max: u32, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    };
    let spec = |name: &str, params: Vec<ParamSpec>| OpSpec {
        name: name.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        kind: OpKind::Write,
        params,
        output_fields: [
            "tenant", "state", "pausedAt", "pausedBy", "note", "outcome", "door",
        ]
        .iter()
        .map(|f| FieldSpec::new(*f))
        .collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        spec(PAUSE, vec![text("note", 32, false)]),
        spec(RESUME, Vec::new()),
        spec(
            PAUSE_TENANT,
            vec![text("tenant", 256, true), text("note", 32, false)],
        ),
        spec(RESUME_TENANT, vec![text("tenant", 256, true)]),
    ]
}

/// **The platform ops that answer on a paused tenant.**
///
/// The rule behind the list: a paused tenant is still, so nothing new
/// happens inside it. What answers is what lets its people get out, look,
/// or take access away: resume, export, delete, the records of what
/// happened, and the member and sharing acts that only narrow who can
/// reach it. A new member, a share, a bucket, an attestation destination
/// and a leave are all something new happening, and are refused.
pub fn answers_while_paused(name: &str) -> bool {
    use crate::{
        attest_ops, exit_ops, invite_ops, member_ops, platform_ops, share_links, share_ops,
    };
    OPS.contains(&name)
        || name == exit_ops::EXPORT
        || name == exit_ops::DELETE
        || name == attest_ops::CHAIN_PROOF
        || name == attest_ops::ATTESTATION
        || name == attest_ops::ATTESTATION_DEAD_LETTERS
        || name == platform_ops::CALLERS_OF
        || name == platform_ops::MY_TENANTS
        // An owner can still remove a departing employee during a pause.
        || name == member_ops::SET_ROLE
        || name == member_ops::SUSPEND
        || name == member_ops::REINSTATE
        || name == member_ops::REMOVE
        // Who can reach the tenant from outside, and taking that away.
        || name == share_ops::SHARES
        || name == share_ops::REVOKE_SHARE
        || name == share_links::LINKS
        || name == share_links::REVOKE_LINK
        || name == invite_ops::INVITATIONS
        || name == invite_ops::REVOKE_INVITATION
        || name == crate::admission::ADMISSION
        // Who may call what, read back.
        || name == crate::profile_ops::PROFILE
        || name == crate::profile_ops::GROUPS
        // Who is in the tenant and what they may do (G5).
        || crate::people_ops::OPS.contains(&name)
}

/// Is `op` an app op the serving program marks `nopause`?
fn marked_nopause(spaces: &Spaces, op: &str) -> bool {
    spaces
        .deployment()
        .current()
        .fixture
        .registry
        .get(op)
        .is_some_and(|o| o.flags.nopause)
}

/// One paused tenant, as every replica holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PauseRecord {
    pub space: String,
    /// When, in seconds.
    pub at: u64,
    /// [`PAUSED_BY_TENANT`] or [`PAUSED_BY_DEPLOYMENT`]. Never a name.
    pub by: String,
    /// A word from [`PAUSE_NOTES`].
    pub note: String,
}

impl From<&tenants::Tenant> for PauseRecord {
    fn from(t: &tenants::Tenant) -> PauseRecord {
        PauseRecord {
            space: t.id.clone(),
            at: t.paused_at,
            by: t.paused_by.clone(),
            note: t.paused_note.clone(),
        }
    }
}

/// The refusal a paused tenant answers with.
///
/// The default message names the layer and the reason word and nothing else:
/// no person, and no free text. The app developer may set, per reason word,
/// the status and the message ([`PauseAnswers`]); the class stays
/// `space_paused` whatever the status. Who may resume is the app's own screen
/// to say.
pub fn refusal(record: &PauseRecord, answers: &PauseAnswers) -> ExecError {
    let reason = if record.note.is_empty() {
        "other"
    } else {
        record.note.as_str()
    };
    let set = answers.get(reason);
    let message = match set.and_then(|a| a.message.clone()) {
        Some(text) => text,
        None => {
            let by = if record.by == PAUSED_BY_DEPLOYMENT {
                "the deployment"
            } else {
                "the tenant"
            };
            format!(
                "space_paused: this tenant is paused (by {by}, reason `{reason}`, since {}). \
                 Nothing ran. It answers again once it is resumed",
                record.at,
            )
        }
    };
    let mut e = ExecError::new(ErrorCode::SpacePaused, message);
    e.status = set.and_then(|a| a.status);
    e
}

// ── what a paused tenant answers: the app developer's setting ────────────

/// The setting: a JSON object keyed by reason word.
pub const PAUSE_ANSWERS_ENV: &str = "ORDO_TENANT_PAUSE_ANSWERS";

/// The statuses a pause may answer with: any 4xx, and 503.
pub fn status_allowed(status: u16) -> bool {
    (400..=499).contains(&status) || status == 503
}

/// What one reason answers. A field left out keeps today's answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PauseAnswer {
    /// The HTTP status. `None`: 423.
    pub status: Option<u16>,
    /// The message text, the app developer's fixed words. `None`: the
    /// default message.
    pub message: Option<String>,
}

/// **What a paused tenant answers, per reason** (owner, 2026-10-06: *"what it
/// actually is/responds should be up to the app dev. The API makes sense but
/// a message as to way should be overrideable dev."*).
///
/// The class is always `space_paused`. The status is 4xx or 503, 423 unless
/// set. A 503 is still never replayed from the cache. The message is fixed
/// text from the app developer: it never carries a person's name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PauseAnswers {
    by_reason: std::collections::BTreeMap<String, PauseAnswer>,
}

impl PauseAnswers {
    /// From the setting's text, `{"billing": {"status": 402, "message": "..."}}`.
    /// A reason word outside the closed list, a status outside 4xx and 503,
    /// an empty message and any other key refuse.
    pub fn parse(text: &str) -> Result<PauseAnswers, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(PauseAnswers::default());
        }
        let value: serde_json::Value = serde_json::from_str(text).map_err(|e| {
            format!(
                "is not JSON ({e}). Write it as {{\"billing\": {{\"status\": 402, \
                 \"message\": \"Payment is overdue.\"}}}}"
            )
        })?;
        let Some(object) = value.as_object() else {
            return Err("must be a JSON object keyed by reason word".to_string());
        };
        let mut by_reason = std::collections::BTreeMap::new();
        for (reason, body) in object {
            if !PAUSE_NOTES.contains(&reason.as_str()) {
                return Err(format!(
                    "`{reason}` is not a pause reason. The reasons are {}",
                    PAUSE_NOTES.join(", ")
                ));
            }
            let Some(fields) = body.as_object() else {
                return Err(format!(
                    "`{reason}` must be an object with `status` and `message`"
                ));
            };
            let mut answer = PauseAnswer::default();
            for (key, v) in fields {
                match key.as_str() {
                    "status" => {
                        let status = v
                            .as_u64()
                            .and_then(|n| u16::try_from(n).ok())
                            .filter(|n| status_allowed(*n))
                            .ok_or_else(|| {
                                format!(
                                    "`{reason}`: status {v} is not allowed. Use a 4xx status \
                                     or 503"
                                )
                            })?;
                        answer.status = Some(status);
                    }
                    "message" => {
                        let text = v
                            .as_str()
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                            .ok_or_else(|| format!("`{reason}`: message must be non-empty text"))?;
                        answer.message = Some(text.to_string());
                    }
                    other => {
                        return Err(format!(
                            "`{reason}`: `{other}` is not a field. The fields are status \
                             and message"
                        ))
                    }
                }
            }
            by_reason.insert(reason.clone(), answer);
        }
        Ok(PauseAnswers { by_reason })
    }

    /// From [`PAUSE_ANSWERS_ENV`]. Unset: every reason answers as today.
    pub fn from_env() -> Result<PauseAnswers, String> {
        match std::env::var(PAUSE_ANSWERS_ENV) {
            Ok(text) => PauseAnswers::parse(&text),
            Err(_) => Ok(PauseAnswers::default()),
        }
    }

    /// What `reason` answers, if the app developer set it.
    pub fn get(&self, reason: &str) -> Option<&PauseAnswer> {
        self.by_reason.get(reason)
    }
}

/// **The pause gate.** `Ok` when `space` is not paused, or `op` answers on a
/// paused tenant; the `space_paused` refusal otherwise.
///
/// Shaped for one tenant at a time on purpose, so a later deployment-wide
/// layer can ask the same question with its own record in front of it.
pub fn gate(spaces: &Spaces, op: &str, space: &str) -> Result<(), ExecError> {
    let Some(record) = spaces.paused_record(space) else {
        return Ok(());
    };
    if answers_while_paused(op) || marked_nopause(spaces, op) {
        return Ok(());
    }
    Err(refusal(&record, &spaces.pause_answers()))
}

/// Load every replica's pause register from the registry. Returns how many
/// tenants are paused. A deployment with no registry yet has none.
pub async fn reload(spaces: &Spaces, control: &Pool) -> Result<usize, String> {
    if !crate::registrations::table_exists(control, "$spaces").await? {
        spaces.replace_paused(Vec::new());
        return Ok(0);
    }
    let paused = tenants::paused(control).await.map_err(|e| e.to_string())?;
    let n = paused.len();
    spaces.replace_paused(paused.iter().map(PauseRecord::from).collect());
    Ok(n)
}

// ── who may, inside a tenant ─────────────────────────────────────────────

/// The tenant roles that hold `std.space.pause`. Unset or empty: none.
pub const PAUSE_ROLES_ENV: &str = "ORDO_TENANT_PAUSE_ROLES";
/// The tenant roles that hold `std.space.resume`. Unset: the pause roles.
pub const RESUME_ROLES_ENV: &str = "ORDO_TENANT_RESUME_ROLES";

/// **Which tenant roles may pause and resume their own tenant.** The app
/// developer's setting; the default is nobody, which leaves pausing to the
/// deployment owner (owner, 2026-10-05: *"This is an app choice if they want
/// to expose that to the tenant - but its a deployment operation."*).
///
/// A role named here is granted the op at the door like any other grant,
/// so step-up, delegation and `callersOf` treat it the same way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PausePolicy {
    pause: Vec<Role>,
    resume: Vec<Role>,
    answers: PauseAnswers,
}

fn roles(list: &str, var: &str) -> Result<Vec<Role>, String> {
    let mut out = Vec::new();
    for word in list.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        let role = match word {
            "owner" => Role::Owner,
            "manager" => Role::Manager,
            "editor" => Role::Editor,
            "viewer" => Role::Viewer,
            other => {
                return Err(format!(
                    "{var}: `{other}` is not a role. The roles are owner, manager, editor \
                     and viewer"
                ))
            }
        };
        if !out.contains(&role) {
            out.push(role);
        }
    }
    Ok(out)
}

impl PausePolicy {
    /// Only the deployment owner pauses and resumes. The default.
    pub fn deployment_only() -> PausePolicy {
        PausePolicy::default()
    }

    /// From the two lists. `resume` unset follows `pause`.
    pub fn parse(pause: Option<&str>, resume: Option<&str>) -> Result<PausePolicy, String> {
        let pause_roles = roles(pause.unwrap_or(""), PAUSE_ROLES_ENV)?;
        let resume_roles = match resume {
            Some(list) => roles(list, RESUME_ROLES_ENV)?,
            None => pause_roles.clone(),
        };
        Ok(PausePolicy {
            pause: pause_roles,
            resume: resume_roles,
            answers: PauseAnswers::default(),
        })
    }

    /// With what a paused tenant answers, per reason.
    pub fn with_answers(mut self, answers: PauseAnswers) -> PausePolicy {
        self.answers = answers;
        self
    }

    /// What a paused tenant answers, per reason.
    pub fn answers(&self) -> &PauseAnswers {
        &self.answers
    }

    /// From [`PAUSE_ROLES_ENV`] and [`RESUME_ROLES_ENV`].
    pub fn from_env() -> Result<PausePolicy, String> {
        PausePolicy::parse(
            std::env::var(PAUSE_ROLES_ENV).ok().as_deref(),
            std::env::var(RESUME_ROLES_ENV).ok().as_deref(),
        )
    }

    /// The roles granted `op`, for the two tenant-scoped ops; `None` for
    /// any other op.
    pub fn roles_for(&self, op: &str) -> Option<&[Role]> {
        match op {
            PAUSE => Some(&self.pause),
            RESUME => Some(&self.resume),
            _ => None,
        }
    }

    /// The census line.
    pub fn describe(&self) -> String {
        let words = |r: &[Role]| {
            if r.is_empty() {
                "nobody in a tenant".to_string()
            } else {
                r.iter()
                    .map(|r| format!("{r:?}").to_lowercase())
                    .collect::<Vec<_>>()
                    .join(",")
            }
        };
        format!(
            "pause: {}   resume: {}   (the deployment owner always may)   answers set for: {}",
            words(&self.pause),
            words(&self.resume),
            if self.answers.by_reason.is_empty() {
                "no reason".to_string()
            } else {
                self.answers
                    .by_reason
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(",")
            }
        )
    }
}

static INSTALLED: OnceLock<PausePolicy> = OnceLock::new();

/// Install the deployment's policy, once, before any engine is built. A
/// second call is ignored for `crate::presence::install`'s reason.
pub fn install(policy: PausePolicy) {
    let _ = INSTALLED.set(policy);
}

/// The policy engines are built under: what was installed, or the default.
pub fn current() -> PausePolicy {
    INSTALLED.get().cloned().unwrap_or_default()
}

// ── the ops ──────────────────────────────────────────────────────────────

/// The four ops' state.
pub struct PauseOps {
    /// The control database: the registry, and the channel announced on.
    pub control: Pool,
    /// Where every replica's register lives.
    pub spaces: Arc<Spaces>,
    /// The deployment's own space. The deployment owner's two ops answer
    /// only there, and it cannot itself be paused.
    pub home: String,
    pub stream: Option<Arc<Recorder>>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

fn refused(msg: impl Into<String>) -> ExecError {
    ExecError::new(ErrorCode::TenantRefused, msg)
}

fn unavailable(e: impl std::fmt::Display) -> ExecError {
    ExecError::new(
        ErrorCode::SpaceUnavailable,
        format!("the tenant registry could not be read or written ({e})"),
    )
}

fn text<'a>(bound: &'a BoundParams, name: &str) -> Option<&'a str> {
    bound
        .0
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The note, checked against the closed list, or `default` when none was
/// sent.
fn note_of<'a>(bound: &'a BoundParams, default: &'a str) -> Result<&'a str, ExecError> {
    let note = text(bound, "note").unwrap_or(default);
    if PAUSE_NOTES.contains(&note) {
        Ok(note)
    } else {
        Err(refused(format!(
            "pause_note: a pause is recorded with one of {}, never free text, so nothing \
             personal enters the record. Nothing was paused",
            PAUSE_NOTES.join(", ")
        )))
    }
}

impl PauseOps {
    pub async fn execute(
        &self,
        name: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        match name {
            PAUSE => {
                let note = note_of(bound, "owner")?;
                self.pause(space, PAUSED_BY_TENANT, note, caller).await
            }
            RESUME => self.resume(space, false, caller).await,
            PAUSE_TENANT => {
                self.in_home(space)?;
                let target = self.target(bound)?;
                let note = note_of(bound, "other")?;
                self.pause(target, PAUSED_BY_DEPLOYMENT, note, caller).await
            }
            RESUME_TENANT => {
                self.in_home(space)?;
                let target = self.target(bound)?;
                self.resume(target, true, caller).await
            }
            other => Err(ExecError::internal(format!(
                "`{other}` is not one of the pause ops"
            ))),
        }
    }

    /// The deployment owner's ops answer only in the deployment's own space,
    /// as `std.deployment.apply` does: the role list decides a role, and
    /// this decides whose deployment.
    fn in_home(&self, space: &str) -> Result<(), ExecError> {
        if space == self.home {
            Ok(())
        } else {
            // `not_found`, as `std.deployment.apply` answers: the door's
            // `access_denied` is not the executor's to say, and no
            // membership in a tenant makes this call land there.
            Err(ExecError::new(
                ErrorCode::NotFound,
                "pausing a tenant is the deployment's act: this op answers only in the \
                 deployment's own space",
            ))
        }
    }

    fn target<'a>(&self, bound: &'a BoundParams) -> Result<&'a str, ExecError> {
        let t = text(bound, "tenant")
            .ok_or_else(|| ExecError::internal("`$tenant` did not reach the platform op"))?;
        if !ordo_api::well_formed_space(t) {
            return Err(refused(
                "`tenant` is a tenant id: letters, digits, `_` and `-`. Nothing was changed",
            ));
        }
        Ok(t)
    }

    async fn pause(
        &self,
        target: &str,
        by: &str,
        note: &str,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if target == self.home {
            return Err(refused(
                "home_space: the deployment's own space cannot be paused; it is where every \
                 pause is lifted from",
            ));
        }
        let now = (self.now)();
        match tenants::pause(&self.control, target, by, note, now)
            .await
            .map_err(unavailable)?
        {
            Paused::Now(t) => {
                self.spaces.hold_paused(PauseRecord::from(&t));
                self.record(target, "tenant-paused", by, note, caller);
                let door = self.announce(target).await;
                Ok(answer(&t, "paused", door))
            }
            Paused::Already(t) => {
                // The register may not have heard yet; the registry is the
                // record, so this replica takes it now.
                self.spaces.hold_paused(PauseRecord::from(&t));
                Ok(answer(&t, "already paused", "nothing changed"))
            }
            Paused::NotServing(t) => Err(refused(format!(
                "not_serving: a tenant that is {} cannot be paused; there is nothing serving \
                 to hold still",
                t.state.as_str()
            ))),
            Paused::Unknown => Err(ExecError::new(
                ErrorCode::NotFound,
                "this deployment has no record of that tenant; only a tenant it minted can be \
                 paused",
            )),
        }
    }

    async fn resume(
        &self,
        target: &str,
        by_deployment: bool,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let now = (self.now)();
        match tenants::resume(&self.control, target, by_deployment, now)
            .await
            .map_err(unavailable)?
        {
            Resumed::Now(t) => {
                self.spaces.lift_paused(target);
                let by = if by_deployment {
                    PAUSED_BY_DEPLOYMENT
                } else {
                    PAUSED_BY_TENANT
                };
                self.record(target, "tenant-resumed", by, "", caller);
                let door = self.announce(target).await;
                Ok(answer(&t, "resumed", door))
            }
            Resumed::NotPaused(t) => {
                self.spaces.lift_paused(target);
                Ok(answer(&t, "not paused", "nothing changed"))
            }
            Resumed::HeldByDeployment(_) => Err(refused(
                "paused_by_deployment: the deployment owner paused this tenant, and only the \
                 deployment owner resumes it",
            )),
            Resumed::Unknown => Err(ExecError::new(
                ErrorCode::NotFound,
                "this deployment has no record of that tenant",
            )),
        }
    }

    /// On the tenant's stream and the deployment's, with the layer and the
    /// reason word. The caller is named as a member or principal id, which
    /// every act on the record carries; never a display name.
    fn record(&self, target: &str, act: &str, by: &str, note: &str, caller: &str) {
        let Some(stream) = &self.stream else {
            return;
        };
        let mut detail = format!("tenant={target} by={by}");
        if !note.is_empty() {
            detail.push_str(&format!(" note={note}"));
        }
        crate::enrolment::record(
            stream,
            &crate::enrolment::Act {
                act,
                detail,
                by: Some(caller),
                tenant: Some(target),
                deployment: true,
                at: (self.now)(),
            },
        );
    }

    /// Tell every other replica to reload its register.
    async fn announce(&self, target: &str) -> &'static str {
        match crate::deploy::announce_members(&self.control, target).await {
            Ok(()) => "held here, announced to every replica",
            Err(_) => {
                "held here, and the announcement failed: other replicas follow at their next \
                 refresh"
            }
        }
    }
}

fn answer(t: &tenants::Tenant, outcome: &str, door: &str) -> ExecResult {
    let paused = t.state == tenants::State::Paused;
    ExecResult {
        position: None,
        rows: vec![json!({
            "tenant": t.id,
            "state": t.state.as_str(),
            "pausedAt": if paused { json!(t.paused_at) } else { json!(null) },
            "pausedBy": if paused { json!(t.paused_by) } else { json!(null) },
            "note": if paused { json!(t.paused_note) } else { json!(null) },
            "outcome": outcome,
            "door": door,
        })],
        next_cursor: None,
    }
}

/// **What a person's tenant list says about a tenant's pause** (27ar):
/// `state` is `active` or `paused`, and a paused one carries `pausedAt`,
/// `pausedBy` and `note`. Written into a row of `std.person.tenants` and of
/// `GET /auth/spaces`, so both lists say the same thing. A tenant the
/// registry does not hold (the deployment's own space) reads `active`.
pub fn list_state(row: &mut serde_json::Value, tenant: Option<&tenants::Tenant>) {
    match tenant {
        Some(t) if t.state == tenants::State::Paused => {
            row["state"] = json!("paused");
            row["pausedAt"] = json!(t.paused_at);
            row["pausedBy"] = json!(t.paused_by);
            row["note"] = json!(t.paused_note);
        }
        _ => row["state"] = json!("active"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_default_no_tenant_role_may_pause_or_resume() {
        let p = PausePolicy::deployment_only();
        assert_eq!(p.roles_for(PAUSE), Some(&[][..]));
        assert_eq!(p.roles_for(RESUME), Some(&[][..]));
        assert_eq!(p.roles_for(PAUSE_TENANT), None);
    }

    #[test]
    fn resume_roles_follow_the_pause_roles_unless_set() {
        let p = PausePolicy::parse(Some("owner, manager"), None).expect("parses");
        assert_eq!(p.roles_for(RESUME), Some(&[Role::Owner, Role::Manager][..]));
        let p = PausePolicy::parse(Some("owner,manager"), Some("owner")).expect("parses");
        assert_eq!(p.roles_for(RESUME), Some(&[Role::Owner][..]));
        let p = PausePolicy::parse(Some("owner"), Some("")).expect("parses");
        assert_eq!(p.roles_for(RESUME), Some(&[][..]));
    }

    #[test]
    fn a_word_that_is_not_a_role_refuses_the_setting() {
        let e = PausePolicy::parse(Some("owner,admin"), None).unwrap_err();
        assert!(e.contains("`admin` is not a role"), "{e}");
    }

    #[test]
    fn leaving_sharing_and_admitting_are_refused_while_paused() {
        for op in [
            crate::release_ops::LEAVE,
            crate::share_ops::SHARE,
            crate::invite_ops::INVITE,
            crate::member_ops::ADMIT,
            crate::bucket_ops::ATTACH_BUCKET,
            crate::bucket_ops::DETACH_BUCKET,
            crate::attest_ops::ATTEST_TO,
            crate::exit_ops::IMPORT,
            "std.invoices.create",
        ] {
            assert!(!answers_while_paused(op), "{op}");
        }
    }

    #[test]
    fn export_delete_resume_and_the_member_commands_answer_while_paused() {
        for op in [
            RESUME,
            crate::exit_ops::EXPORT,
            crate::exit_ops::DELETE,
            crate::member_ops::REMOVE,
            crate::member_ops::SUSPEND,
            crate::platform_ops::MY_TENANTS,
        ] {
            assert!(answers_while_paused(op), "{op}");
        }
    }

    #[test]
    fn the_refusal_names_the_layer_and_the_reason_and_no_person() {
        let e = refusal(
            &PauseRecord {
                space: "spc_x".into(),
                at: 42,
                by: PAUSED_BY_DEPLOYMENT.into(),
                note: "billing".into(),
            },
            &PauseAnswers::default(),
        );
        assert_eq!(e.code, ErrorCode::SpacePaused);
        assert_eq!(e.status, None, "the default is 423");
        assert!(e.message.contains("by the deployment"), "{}", e.message);
        assert!(e.message.contains("`billing`"), "{}", e.message);
    }
}

#[cfg(test)]
mod answers_tests {
    use super::*;

    fn record(note: &str) -> PauseRecord {
        PauseRecord {
            space: "spc_x".into(),
            at: 42,
            by: PAUSED_BY_DEPLOYMENT.into(),
            note: note.into(),
        }
    }

    #[test]
    fn a_reason_the_app_developer_set_answers_their_status_and_message() {
        let a = PauseAnswers::parse(
            r#"{"billing": {"status": 402, "message": "Payment is overdue."}}"#,
        )
        .expect("parses");
        let e = refusal(&record("billing"), &a);
        assert_eq!(e.status, Some(402));
        assert_eq!(e.message, "Payment is overdue.");
        let other = refusal(&record("abuse"), &a);
        assert_eq!(other.status, None, "a reason not set keeps 423");
        assert!(other.message.contains("`abuse`"));
    }

    #[test]
    fn a_reason_word_outside_the_list_is_refused() {
        let e = PauseAnswers::parse(r#"{"nonpayment": {"status": 402}}"#).unwrap_err();
        assert!(e.contains("`nonpayment` is not a pause reason"), "{e}");
    }

    #[test]
    fn a_status_outside_4xx_and_503_is_refused() {
        for bad in ["200", "302", "500", "502", "600", "70000", "\"402\""] {
            let text = format!(r#"{{"billing": {{"status": {bad}}}}}"#);
            assert!(PauseAnswers::parse(&text).is_err(), "{bad}");
        }
        for good in ["400", "402", "423", "451", "503"] {
            let text = format!(r#"{{"billing": {{"status": {good}}}}}"#);
            assert!(PauseAnswers::parse(&text).is_ok(), "{good}");
        }
    }

    #[test]
    fn an_empty_message_and_an_unknown_field_are_refused() {
        assert!(PauseAnswers::parse(r#"{"billing": {"message": "  "}}"#).is_err());
        assert!(PauseAnswers::parse(r#"{"billing": {"body": "x"}}"#).is_err());
        assert!(PauseAnswers::parse("not json").is_err());
        assert_eq!(PauseAnswers::parse("").unwrap(), PauseAnswers::default());
    }
}
