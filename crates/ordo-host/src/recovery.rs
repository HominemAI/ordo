//! Credential recovery: the way back in when every passkey is gone
//! (`plan-passkey-recovery.md` R3).
//!
//! # What is here
//!
//! A **case** is one recovery: a row in `$recovery` that says who is being
//! recovered, by which path, on whose authority and on what evidence, when it
//! matures, and what state it is in. A case that has matured yields one
//! **recovery ticket**, the enrolment ticket of E2 with the purpose
//! [`Purpose::Recovery`]. The person redeems it by registering a new passkey
//! at `/auth/recover/complete`, and only that: no endpoint that takes a
//! password asks for this purpose.
//!
//! Completing a recovery does these things, in this order, and stops at the
//! first that fails: the case is locked, the directory recovers
//! ([`ordo_identity::Directory::recover`], which shares no function with
//! `link`), the ticket is spent, every session of the person ends, the
//! second-factor settlements run, and the acts are recorded at deployment
//! scope and mirrored into every tenant the person belongs to.
//!
//! # The contest window
//!
//! The authenticators the recovery retired are kept, not deleted, for the
//! window. An assertion from one of them can do exactly one thing:
//! [`Recovery::contest`], which removes what the recovery added, restores what
//! it retired, ends every session again and records who contested. Someone
//! who really lost a phone never contests, and at the end of the window the
//! retired links and their public keys are removed by [`Recovery::sweep`].
//!
//! # One live case per person
//!
//! A person has at most one case in `pending`, `matured` or `completed`. That
//! is a unique index and not a check, so two operators opening a case for the
//! same person at once cannot both succeed. It also means a ticket names its
//! case by naming its person, which is why a ticket needs no case column.
//!
//! # What is configurable
//!
//! [`RecoveryPolicy`]. The defaults are the safe ones and a weaker value is a
//! setting the deployment owner makes on purpose: the quarantine is one day
//! (owner, 2026-09-29: a day's block after recovery, adjustable) and the
//! contest window is three.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ordo_identity::directory::random_principal_suffix;
use ordo_identity::principal::PrincipalId;
use ordo_identity::provider::AuthenticatorId;
use ordo_identity::session::SessionRevocations;
use ordo_identity::{Contested, Directory, DirectoryError, RecoveryTicket, Timestamp};
use ordo_secevent::{EventKind, Observation, Recorder, Relationship, Subject};

use crate::auth::store_failure::IntoFault;
use crate::pgsync::{cell, PgSync, Source};
use crate::tickets::{Purpose, TicketSpec, Tickets, Want};
use ordo_exec::{sql_lit, Pool, PoolConfig, PoolError};
use ordo_identity::StoreFault;

/// The word every case id starts with.
pub const CASE_PREFIX: &str = "rec_";

/// How a recovery was started, which decides what it had to prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// One unused recovery code (R4).
    Code,
    /// A tenant's administrator, for a person the tenant created (R6).
    Tenant,
    /// The deployment owner, through the door (R5).
    Operator,
    /// The deployment owner at the host, with the database credentials.
    Host,
    /// Not a recovery. The deployment owner makes an existing member the
    /// owner of a tenant whose owner is gone for good
    /// (`std.space.succeedOwner`). It takes a case for the reason a recovery
    /// does, the delay and the record, and it touches no credential.
    Succession,
}

impl Path {
    pub fn as_str(self) -> &'static str {
        match self {
            Path::Code => "code",
            Path::Tenant => "tenant",
            Path::Operator => "operator",
            Path::Host => "host",
            Path::Succession => "succession",
        }
    }

    fn parse(s: &str) -> Option<Path> {
        match s {
            "code" => Some(Path::Code),
            "tenant" => Some(Path::Tenant),
            "operator" => Some(Path::Operator),
            "host" => Some(Path::Host),
            "succession" => Some(Path::Succession),
            _ => None,
        }
    }
}

/// Where a case is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Opened, and the delay before it may yield a ticket has not passed.
    Pending,
    /// A ticket may be redeemed (or was issued at open, when there is no
    /// delay).
    Matured,
    /// Redeemed: the person is recovered and the contest window is open.
    Completed,
    /// Ended before completion, or contested after it.
    Cancelled,
    /// The contest window ended and the retired links were removed.
    Closed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Matured => "matured",
            State::Completed => "completed",
            State::Cancelled => "cancelled",
            State::Closed => "closed",
        }
    }

    fn parse(s: &str) -> Option<State> {
        match s {
            "pending" => Some(State::Pending),
            "matured" => Some(State::Matured),
            "completed" => Some(State::Completed),
            "cancelled" => Some(State::Cancelled),
            "closed" => Some(State::Closed),
            _ => None,
        }
    }
}

/// One recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    pub id: String,
    pub principal: String,
    pub path: Path,
    /// Who opened it: a principal id, or a word for the host.
    pub opened_by: String,
    /// The class of authority, a fixed word: `host`, `deployment-owner`,
    /// `tenant-administrator`, `recovery-code`.
    pub authority: String,
    /// The class of evidence, from a closed list. Never free text.
    pub evidence: String,
    /// An opaque reference into the opener's own ticketing system.
    pub reference: String,
    pub opened_at: u64,
    pub matures_at: u64,
    /// Zero until the case completes.
    pub contest_until: u64,
    pub state: State,
    /// The tenant a succession is about. Empty for a recovery.
    pub space: String,
    /// The person has seen that this recovery happened and said so, with
    /// presence. Until then every sign-in of theirs carries the case.
    pub acknowledged: bool,
    /// How many approvals the case needs before it may yield a ticket, the
    /// opener's included. One for a case nobody else has to agree to.
    pub approvals_required: u32,
    /// A succession's departed owners, as the deployment owner named them
    /// when the case opened, comma separated. Completing the
    /// succession ends their grants and delegations in the tenant when the
    /// option is on. Empty for a recovery.
    pub departed: String,
}

/// A manager's objection to a case about their tenant's owner (owner,
/// 2026-10-24, `plan-passkey-recovery.md` ask 8). While one is unresolved the
/// case is **paused**: it does not mature, yields no ticket and cannot be
/// carried out, until the deployment owner resolves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Objection {
    pub case: String,
    /// The manager's principal.
    pub objector: String,
    /// The tenant they manage, where they objected.
    pub space: String,
    pub at: u64,
    /// Zero until the deployment owner resolves it.
    pub resolved_at: u64,
    /// `resume` or `cancel`, empty until resolved.
    pub resolution: String,
    /// A word from [`RESUME_REASONS`] or [`CANCEL_REASONS_ON_OBJECTION`].
    pub reason: String,
    pub resolved_by: String,
}

/// What the deployment owner decides about an objection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// The case goes on from where it was paused.
    Resume,
    /// The case ends.
    Cancel,
}

impl Resolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Resolution::Resume => "resume",
            Resolution::Cancel => "cancel",
        }
    }

    pub fn parse(s: &str) -> Option<Resolution> {
        match s {
            "resume" => Some(Resolution::Resume),
            "cancel" => Some(Resolution::Cancel),
            _ => None,
        }
    }

    /// The reasons this decision may give. A short fixed list, so no free
    /// text enters the stream.
    pub fn reasons(self) -> &'static [&'static str] {
        match self {
            Resolution::Resume => RESUME_REASONS,
            Resolution::Cancel => CANCEL_REASONS_ON_OBJECTION,
        }
    }
}

/// Why a paused case may go on.
pub const RESUME_REASONS: &[&str] = &[
    "evidence-rechecked",
    "objector-withdrew",
    "objector-answered",
];

/// Why a paused case may end.
pub const CANCEL_REASONS_ON_OBJECTION: &[&str] =
    &["objection-upheld", "opened-in-error", "not-verified"];

/// Why the deployment owner may close a recovery's contest window early
/// (`std.identity.closeContest`). A short fixed list, so no free text enters
/// the stream.
///
/// - `recovery-verified`: the person who recovered is the owner, and a thief
///   holding the lost device keeps cancelling it (`plan-passkey-recovery.md`
///   §4.2, path D).
/// - `recovery-disputed`: the recovery is believed to be someone else's, and
///   the owner needs a new case at once rather than after the window.
/// - `opened-in-error`: the case should never have run.
pub const CLOSE_CONTEST_REASONS: &[&str] =
    &["recovery-verified", "recovery-disputed", "opened-in-error"];

/// What closing a contest window early did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContestClosed {
    pub case: String,
    pub principal: String,
    /// The links the case retired, which can no longer contest. The next
    /// sweep removes them and their public keys.
    pub retired: Vec<AuthenticatorId>,
}

/// The settings a recovery takes. The defaults are the safe values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryPolicy {
    /// How long a recovered passkey may not satisfy step-up. One day by
    /// default (owner, 2026-09-29), adjustable.
    pub quarantine_secs: u64,
    /// How long a retired authenticator may contest. Never shorter than the
    /// quarantine, so a contest can land before the block on step-up lifts.
    pub contest_secs: u64,
    /// The delay before a case opened at the host matures. Zero: whoever
    /// holds the database and the master key is already sovereign over the
    /// deployment, and a delay would restrain nobody.
    pub host_delay_secs: u64,
    /// The delay before a case opened with a recovery code matures. Zero
    /// (owner, 2026-09-29: a recovery code works at once). A deployment
    /// holding data where a day of reads is unacceptable sets one.
    pub code_delay_secs: u64,
    /// The delay before a case the deployment owner opens through the door
    /// matures: a day. Long enough for a person who still holds a passkey to
    /// see the case and cancel it, short enough that a lost phone is not a
    /// week's outage. The plan's default, not yet confirmed by the owner.
    pub operator_delay_secs: u64,
    /// The delay before an owner succession may be carried out. The same day.
    pub succession_delay_secs: u64,
    /// The delay before a case a tenant's owner opens for a member the tenant
    /// created matures. Zero: the ordinary case is an employee at the IT desk,
    /// and a delay there fails the test of being easy to use. The deployment
    /// may raise it, and each tenant may raise it for its own members (R7).
    pub tenant_delay_secs: u64,
    /// Whether a tenant that has not chosen holds a recovered person. Off
    /// (owner, 2026-09-29: a recovered person is not locked out of each
    /// tenant). A tenant chooses for itself with `std.space.setRecoveryPolicy`.
    pub hold_default: bool,
    /// Whether a tenant's managers may recover a member below manager, as its
    /// owners may (owner, 2026-09-29: *"if the app dev sets it that way"*).
    /// Off unless the app developer sets it, `ORDO_RECOVERY_MANAGERS=on`. A
    /// tenant may turn it off for itself with `std.space.setRecoveryPolicy`
    /// and cannot turn it on when this is off.
    pub managers_may_recover: bool,
    /// How many deployment owners must approve a case opened through the
    /// door, or a succession, before it may yield anything. The opener is the
    /// first. One, so a lone deployment owner is not locked out of their own
    /// deployment, and a deployment with several raises it (`plan` G).
    pub operator_approvals: u32,
    /// How many of a tenant's owners must approve a case opened for a member
    /// it created. The opener is the first. A tenant may raise it for itself,
    /// and may not lower this.
    pub tenant_approvals: u32,
    /// The paths this deployment allows. All four by default. Turning one off
    /// is a decision somebody made, and the boot prints it.
    pub paths: Vec<Path>,
    /// How long a recovery ticket lives. `ORDO_RECOVERY_TICKET_SECS`, from
    /// 1 hour to 30 days, default 3 days.
    pub ticket_ttl_secs: u64,
    /// How many codes a recovery-code set holds.
    /// `ORDO_RECOVERY_CODES_PER_SET`, from 1 to 50, default 10.
    pub codes_per_set: usize,
    /// Whether completing an owner succession ends the departed owner's
    /// grants to apps and their delegations in the tenant, when the
    /// completing call does not say. On (owner, 2026-10-24): the new owner
    /// can grant again. `ORDO_SUCCESSION_ENDS_GRANTS=off` keeps them.
    pub succession_ends_grants: bool,
}

/// The shortest quarantine a deployment may set, and the shortest delay it may
/// put on a case opened through the door or on a succession. A floor is what a
/// lower setting is refused against: a quarantine of nothing would make
/// step-up exactly as strong as the weakest recovery path.
pub const FLOOR_SECS: u64 = 3600;
/// The most approvals a setting may ask for.
pub const MAX_APPROVALS: u32 = 10;

impl Default for RecoveryPolicy {
    fn default() -> RecoveryPolicy {
        RecoveryPolicy {
            quarantine_secs: 24 * 3600,
            contest_secs: 72 * 3600,
            host_delay_secs: 0,
            code_delay_secs: 0,
            operator_delay_secs: 24 * 3600,
            succession_delay_secs: 24 * 3600,
            tenant_delay_secs: 0,
            hold_default: false,
            managers_may_recover: false,
            operator_approvals: 1,
            tenant_approvals: 1,
            paths: vec![Path::Code, Path::Tenant, Path::Operator, Path::Host],
            ticket_ttl_secs: 3 * 24 * 3600,
            codes_per_set: crate::recovery_codes::SET_SIZE,
            succession_ends_grants: true,
        }
    }
}

/// A recovery setting that is refused, and why. Names the variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyError {
    pub var: &'static str,
    pub detail: String,
}

impl RecoveryPolicy {
    /// Is this path allowed? A succession follows the deployment owner's
    /// path, since it is that person's act.
    pub fn enabled(&self, path: Path) -> bool {
        let asked = if path == Path::Succession {
            Path::Operator
        } else {
            path
        };
        self.paths.contains(&asked)
    }

    /// How many approvals a case of this path needs at the least.
    pub fn approvals_for(&self, path: Path) -> u32 {
        match path {
            Path::Operator | Path::Succession => self.operator_approvals,
            Path::Tenant => self.tenant_approvals,
            Path::Code | Path::Host => 1,
        }
        .max(1)
    }

    /// Read the deployment's settings from the environment, refusing what is
    /// unparseable or below its floor. See [`RecoveryPolicy::from_lookup`].
    pub fn from_env() -> Result<RecoveryPolicy, PolicyError> {
        RecoveryPolicy::from_lookup(&|k| std::env::var(k).ok())
    }

    /// The settings, read through `get` so a test can supply them. **Each is
    /// refused rather than clamped**: an unparseable value, one below its
    /// floor, a quarantine longer than the contest window, and an approvals
    /// count outside 1 to 10 each name their variable and the rule they broke.
    /// A setting that quietly became a different number is a security setting
    /// nobody chose.
    pub fn from_lookup(
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<RecoveryPolicy, PolicyError> {
        let d = RecoveryPolicy::default();
        let secs = |var: &'static str, default: u64, floor: u64| -> Result<u64, PolicyError> {
            let Some(raw) = get(var)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
            else {
                return Ok(default);
            };
            let n: u64 = raw.parse().map_err(|_| PolicyError {
                var,
                detail: format!("`{raw}` is not a number of seconds"),
            })?;
            if n < floor {
                return Err(PolicyError {
                    var,
                    detail: format!(
                        "{n} seconds is below the floor of {floor}. A setting weaker than the \
                         floor would make a recovery as strong as its weakest proof"
                    ),
                });
            }
            Ok(n)
        };
        // A number from `min` to `max`, or the default; a bad one names the
        // setting and the default.
        let bounded = |var: &'static str, default: u64, min: u64, max: u64| {
            let Some(raw) = get(var)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
            else {
                return Ok(default);
            };
            match raw.parse::<u64>() {
                Ok(n) if (min..=max).contains(&n) => Ok(n),
                _ => Err(PolicyError {
                    var,
                    detail: format!(
                        "`{raw}` is not a number from {min} to {max} (default {default})"
                    ),
                }),
            }
        };
        let approvals = |var: &'static str, default: u32| -> Result<u32, PolicyError> {
            let Some(raw) = get(var)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
            else {
                return Ok(default);
            };
            let n: u32 = raw.parse().map_err(|_| PolicyError {
                var,
                detail: format!("`{raw}` is not a number"),
            })?;
            if !(1..=MAX_APPROVALS).contains(&n) {
                return Err(PolicyError {
                    var,
                    detail: format!("{n} approvals is not between 1 and {MAX_APPROVALS}"),
                });
            }
            Ok(n)
        };
        let quarantine = secs(
            "ORDO_RECOVERY_QUARANTINE_SECS",
            d.quarantine_secs,
            FLOOR_SECS,
        )?;
        let contest = secs("ORDO_RECOVERY_CONTEST_SECS", d.contest_secs, FLOOR_SECS)?;
        if quarantine > contest {
            return Err(PolicyError {
                var: "ORDO_RECOVERY_QUARANTINE_SECS",
                detail: format!(
                    "a quarantine of {quarantine} seconds is longer than the contest window of \
                     {contest}. A person must be able to contest before the block on step-up \
                     lifts, so the quarantine may not be longer than ORDO_RECOVERY_CONTEST_SECS"
                ),
            });
        }
        let paths = match get("ORDO_RECOVERY_PATHS")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            None => d.paths.clone(),
            Some(list) => {
                let mut out = Vec::new();
                for word in list.split(',').map(str::trim).filter(|w| !w.is_empty()) {
                    match Path::parse(word) {
                        Some(p) if p != Path::Succession => {
                            if !out.contains(&p) {
                                out.push(p);
                            }
                        }
                        _ => {
                            return Err(PolicyError {
                                var: "ORDO_RECOVERY_PATHS",
                                detail: format!(
                                    "`{word}` is not a recovery path (code, tenant, operator, host)"
                                ),
                            })
                        }
                    }
                }
                out
            }
        };
        let hold_default = match get("ORDO_RECOVERY_HOLD_DEFAULT")
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref()
        {
            None | Some("") | Some("off") => false,
            Some("on") => true,
            Some(other) => {
                return Err(PolicyError {
                    var: "ORDO_RECOVERY_HOLD_DEFAULT",
                    detail: format!("`{other}` is not `on` or `off`"),
                })
            }
        };
        let managers_may_recover = match get("ORDO_RECOVERY_MANAGERS")
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref()
        {
            None | Some("") | Some("off") => false,
            Some("on") => true,
            Some(other) => {
                return Err(PolicyError {
                    var: "ORDO_RECOVERY_MANAGERS",
                    detail: format!("`{other}` is not `on` or `off`"),
                })
            }
        };
        let succession_ends_grants = match get("ORDO_SUCCESSION_ENDS_GRANTS")
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref()
        {
            None | Some("") | Some("on") => true,
            Some("off") => false,
            Some(other) => {
                return Err(PolicyError {
                    var: "ORDO_SUCCESSION_ENDS_GRANTS",
                    detail: format!("`{other}` is not `on` or `off` (default on)"),
                })
            }
        };
        Ok(RecoveryPolicy {
            succession_ends_grants,
            quarantine_secs: quarantine,
            contest_secs: contest,
            host_delay_secs: secs("ORDO_RECOVERY_HOST_DELAY_SECS", d.host_delay_secs, 0)?,
            code_delay_secs: secs("ORDO_RECOVERY_CODE_DELAY_SECS", d.code_delay_secs, 0)?,
            operator_delay_secs: secs(
                "ORDO_RECOVERY_OPERATOR_DELAY_SECS",
                d.operator_delay_secs,
                FLOOR_SECS,
            )?,
            succession_delay_secs: secs(
                "ORDO_RECOVERY_SUCCESSION_DELAY_SECS",
                d.succession_delay_secs,
                FLOOR_SECS,
            )?,
            tenant_delay_secs: secs("ORDO_RECOVERY_TENANT_DELAY_SECS", d.tenant_delay_secs, 0)?,
            hold_default,
            managers_may_recover,
            operator_approvals: approvals("ORDO_RECOVERY_OPERATOR_APPROVALS", 1)?,
            tenant_approvals: approvals("ORDO_RECOVERY_TENANT_APPROVALS", 1)?,
            paths,
            ticket_ttl_secs: bounded(
                "ORDO_RECOVERY_TICKET_SECS",
                d.ticket_ttl_secs,
                FLOOR_SECS,
                30 * 24 * 3600,
            )?,
            codes_per_set: bounded("ORDO_RECOVERY_CODES_PER_SET", d.codes_per_set as u64, 1, 50)?
                as usize,
        })
    }

    /// One line for the startup log, so what a deployment settled on is read
    /// there and not inferred.
    pub fn describe(&self) -> String {
        let hours = |s: u64| {
            if s.is_multiple_of(3600) {
                format!("{}h", s / 3600)
            } else {
                format!("{s}s")
            }
        };
        format!(
            "paths {}; quarantine {}, contest {}; delay: door {}, host {}, code {}, tenant {}; \
             approvals: door {}, tenant {}; hold by default {}; managers recover members {}; \
             succession ends grants {}",
            self.paths
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(","),
            hours(self.quarantine_secs),
            hours(self.contest_secs),
            hours(self.operator_delay_secs),
            hours(self.host_delay_secs),
            hours(self.code_delay_secs),
            hours(self.tenant_delay_secs),
            self.operator_approvals,
            self.tenant_approvals,
            if self.hold_default { "on" } else { "off" },
            if self.managers_may_recover {
                "on"
            } else {
                "off"
            },
            if self.succession_ends_grants {
                "on"
            } else {
                "off"
            },
        )
    }

    /// The delay a path adds before its case matures.
    pub fn delay_secs(&self, path: Path) -> u64 {
        match path {
            Path::Host => self.host_delay_secs,
            Path::Code => self.code_delay_secs,
            Path::Operator => self.operator_delay_secs,
            Path::Succession => self.succession_delay_secs,
            Path::Tenant => self.tenant_delay_secs,
        }
    }
}

// ── the store ────────────────────────────────────────────────────────────

/// Why a case could not be inserted.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InsertError {
    /// The person already has a live case.
    Live,
    Unavailable(StoreFault),
}

/// Where cases are kept.
pub trait RecoveryStore: Send + Sync {
    /// Insert a case. Refuses ([`InsertError::Live`]) a person who already
    /// has one in `pending`, `matured` or `completed`.
    fn insert(&self, case: &Case) -> Result<(), InsertError>;

    fn get(&self, id: &str) -> Result<Option<Case>, StoreFault>;

    /// The person's live case, if any.
    fn live_for(&self, principal: &str) -> Result<Option<Case>, StoreFault>;

    /// Move a case from any of `from` to `to`, atomically, setting
    /// `contest_until` when given. `false` means it was not in one of the
    /// `from` states and nothing changed.
    fn transition(
        &self,
        id: &str,
        from: &[State],
        to: State,
        contest_until: Option<u64>,
    ) -> Result<bool, StoreFault>;

    /// Close every completed case whose contest window ended at or before
    /// `now`, and return them.
    fn close_expired(&self, now: u64) -> Result<Vec<Case>, StoreFault>;

    /// The person's recoveries they have not yet acknowledged: completed or
    /// closed, not a succession, oldest first.
    fn unacknowledged(&self, principal: &str) -> Result<Vec<Case>, StoreFault>;

    /// Record that the person acknowledged a recovery. `false` when the case
    /// is not theirs, not completed or closed, or already acknowledged.
    fn acknowledge(&self, id: &str, principal: &str, at: u64) -> Result<bool, StoreFault>;

    /// Record an approval. `false` when this approver has already approved
    /// this case.
    fn add_approval(&self, id: &str, approver: &str, at: u64) -> Result<bool, StoreFault>;

    /// Who has approved a case, in the order they did.
    fn approvals(&self, id: &str) -> Result<Vec<String>, StoreFault>;

    /// Every case through the door or a succession that is still `pending`
    /// or `matured`, oldest first: what a manager may be told of and object
    /// to, and what the console lists.
    fn open_cases(&self) -> Result<Vec<Case>, StoreFault>;

    /// Record an objection. `false` when this manager has already objected
    /// to this case.
    fn add_objection(&self, o: &Objection) -> Result<bool, StoreFault>;

    /// Every objection to a case, resolved or not, in the order made.
    fn objections(&self, case: &str) -> Result<Vec<Objection>, StoreFault>;

    /// Resolve every unresolved objection to a case at once, and answer how
    /// many there were.
    fn resolve_objections(
        &self,
        case: &str,
        resolution: Resolution,
        reason: &str,
        by: &str,
        at: u64,
    ) -> Result<u32, StoreFault>;
}

fn live(state: State) -> bool {
    matches!(state, State::Pending | State::Matured | State::Completed)
}

/// The process-local store, for tests and for nothing a deployment runs.
#[derive(Default)]
pub struct MemoryRecoveryStore {
    rows: Mutex<BTreeMap<String, Case>>,
    approvals: Mutex<BTreeMap<String, Vec<String>>>,
    objections: Mutex<Vec<Objection>>,
}

impl MemoryRecoveryStore {
    pub fn new() -> MemoryRecoveryStore {
        MemoryRecoveryStore::default()
    }
}

impl RecoveryStore for MemoryRecoveryStore {
    fn insert(&self, case: &Case) -> Result<(), InsertError> {
        let mut rows = self.rows.lock().expect("recovery store");
        if rows
            .values()
            .any(|c| c.principal == case.principal && live(c.state))
        {
            return Err(InsertError::Live);
        }
        rows.insert(case.id.clone(), case.clone());
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<Case>, StoreFault> {
        Ok(self.rows.lock().expect("recovery store").get(id).cloned())
    }

    fn live_for(&self, principal: &str) -> Result<Option<Case>, StoreFault> {
        Ok(self
            .rows
            .lock()
            .expect("recovery store")
            .values()
            .find(|c| c.principal == principal && live(c.state))
            .cloned())
    }

    fn transition(
        &self,
        id: &str,
        from: &[State],
        to: State,
        contest_until: Option<u64>,
    ) -> Result<bool, StoreFault> {
        let mut rows = self.rows.lock().expect("recovery store");
        match rows.get_mut(id) {
            Some(c) if from.contains(&c.state) => {
                c.state = to;
                if let Some(u) = contest_until {
                    c.contest_until = u;
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn close_expired(&self, now: u64) -> Result<Vec<Case>, StoreFault> {
        let mut rows = self.rows.lock().expect("recovery store");
        let mut closed = Vec::new();
        for c in rows.values_mut() {
            if c.state == State::Completed && c.contest_until <= now {
                c.state = State::Closed;
                closed.push(c.clone());
            }
        }
        Ok(closed)
    }

    fn unacknowledged(&self, principal: &str) -> Result<Vec<Case>, StoreFault> {
        let mut out: Vec<Case> = self
            .rows
            .lock()
            .expect("recovery store")
            .values()
            .filter(|c| {
                c.principal == principal
                    && c.path != Path::Succession
                    && !c.acknowledged
                    && matches!(c.state, State::Completed | State::Closed)
            })
            .cloned()
            .collect();
        out.sort_by_key(|c| c.opened_at);
        Ok(out)
    }

    fn add_approval(&self, id: &str, approver: &str, _at: u64) -> Result<bool, StoreFault> {
        let mut all = self.approvals.lock().expect("recovery store");
        let list = all.entry(id.to_string()).or_default();
        if list.iter().any(|a| a == approver) {
            return Ok(false);
        }
        list.push(approver.to_string());
        Ok(true)
    }

    fn approvals(&self, id: &str) -> Result<Vec<String>, StoreFault> {
        Ok(self
            .approvals
            .lock()
            .expect("recovery store")
            .get(id)
            .cloned()
            .unwrap_or_default())
    }

    fn open_cases(&self) -> Result<Vec<Case>, StoreFault> {
        let mut out: Vec<Case> = self
            .rows
            .lock()
            .expect("recovery store")
            .values()
            .filter(|c| {
                matches!(c.path, Path::Operator | Path::Succession)
                    && matches!(c.state, State::Pending | State::Matured)
            })
            .cloned()
            .collect();
        out.sort_by_key(|c| c.opened_at);
        Ok(out)
    }

    fn add_objection(&self, o: &Objection) -> Result<bool, StoreFault> {
        let mut all = self.objections.lock().expect("recovery store");
        if all
            .iter()
            .any(|x| x.case == o.case && x.objector == o.objector)
        {
            return Ok(false);
        }
        all.push(o.clone());
        Ok(true)
    }

    fn objections(&self, case: &str) -> Result<Vec<Objection>, StoreFault> {
        Ok(self
            .objections
            .lock()
            .expect("recovery store")
            .iter()
            .filter(|o| o.case == case)
            .cloned()
            .collect())
    }

    fn resolve_objections(
        &self,
        case: &str,
        resolution: Resolution,
        reason: &str,
        by: &str,
        at: u64,
    ) -> Result<u32, StoreFault> {
        let mut n = 0;
        for o in self
            .objections
            .lock()
            .expect("recovery store")
            .iter_mut()
            .filter(|o| o.case == case && o.resolved_at == 0)
        {
            o.resolved_at = at.max(1);
            o.resolution = resolution.as_str().to_string();
            o.reason = reason.to_string();
            o.resolved_by = by.to_string();
            n += 1;
        }
        Ok(n)
    }

    fn acknowledge(&self, id: &str, principal: &str, _at: u64) -> Result<bool, StoreFault> {
        let mut rows = self.rows.lock().expect("recovery store");
        match rows.get_mut(id) {
            Some(c)
                if c.principal == principal
                    && !c.acknowledged
                    && matches!(c.state, State::Completed | State::Closed) =>
            {
                c.acknowledged = true;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// The case table. The partial unique index is what makes "one live case per
/// person" a constraint the database enforces.
pub const RECOVERY_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$recovery" (
  "id"            text   NOT NULL,
  "principal"     text   NOT NULL,
  "path"          text   NOT NULL,
  "opened_by"     text   NOT NULL DEFAULT '',
  "authority"     text   NOT NULL DEFAULT '',
  "evidence"      text   NOT NULL DEFAULT '',
  "reference"     text   NOT NULL DEFAULT '',
  "opened_at"     bigint NOT NULL,
  "matures_at"    bigint NOT NULL,
  "contest_until" bigint NOT NULL DEFAULT 0,
  "state"         text   NOT NULL,
  "space"         text   NOT NULL DEFAULT '',
  "acknowledged_at" bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("id")
)"#;

/// The columns a case gained after the table first existed. `ADD COLUMN IF
/// NOT EXISTS`, so a database that holds the table gains them at the next
/// boot.
pub const RECOVERY_ADDED_DDL: [&str; 4] = [
    r#"ALTER TABLE "$recovery" ADD COLUMN IF NOT EXISTS "approvals_required" integer NOT NULL DEFAULT 1"#,
    r#"ALTER TABLE "$recovery" ADD COLUMN IF NOT EXISTS "space" text NOT NULL DEFAULT ''"#,
    r#"ALTER TABLE "$recovery" ADD COLUMN IF NOT EXISTS "acknowledged_at" bigint NOT NULL DEFAULT 0"#,
    r#"ALTER TABLE "$recovery" ADD COLUMN IF NOT EXISTS "departed" text NOT NULL DEFAULT ''"#,
];

/// A manager's objection to a case. `(case, objector)` is the key, so one
/// manager objects to a case once; the deployment owner's resolution is
/// written on the row.
pub const RECOVERY_OBJECTION_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$recoveryobjection" (
  "case"        text   NOT NULL,
  "objector"    text   NOT NULL,
  "space"       text   NOT NULL DEFAULT '',
  "at"          bigint NOT NULL DEFAULT 0,
  "resolved_at" bigint NOT NULL DEFAULT 0,
  "resolution"  text   NOT NULL DEFAULT '',
  "reason"      text   NOT NULL DEFAULT '',
  "resolved_by" text   NOT NULL DEFAULT '',
  PRIMARY KEY ("case", "objector")
)"#;

/// Who approved which case. `(case, approver)` is the key, so one person
/// approves a case once.
pub const RECOVERY_APPROVAL_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$recoveryapproval" (
  "case"     text   NOT NULL,
  "approver" text   NOT NULL,
  "at"       bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("case", "approver")
)"#;

/// A person's sign-in settings as they were when a case opened, one row per
/// setting, so a contest can put back what whoever held the recovered passkey
/// changed (owner, 2026-10-25). `value` is null for a setting the person did
/// not have. Rows older than the settlement's `keep_secs` are pruned when a
/// case opens.
pub const RECOVERY_SNAPSHOT_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$recoverysnapshot" (
  "case"      text   NOT NULL,
  "kind"      text   NOT NULL,
  "principal" text   NOT NULL,
  "value"     text,
  "taken_at"  bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("case", "kind")
)"#;

pub const RECOVERY_LIVE_INDEX_DDL: &str = r#"CREATE UNIQUE INDEX IF NOT EXISTS "$recovery$live"
    ON "$recovery" ("principal")
    WHERE "state" IN ('pending', 'matured', 'completed')"#;

/// Create `$recovery`, retry-once for the reason the other platform tables
/// do.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    for ddl in [RECOVERY_DDL].into_iter().chain(RECOVERY_ADDED_DDL).chain([
        RECOVERY_LIVE_INDEX_DDL,
        RECOVERY_APPROVAL_DDL,
        RECOVERY_OBJECTION_DDL,
        RECOVERY_SNAPSHOT_DDL,
    ]) {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// The Postgres case store.
pub struct PgRecoveryStore {
    db: PgSync,
}

const CASE_COLUMNS: &str = r#""id","principal","path","opened_by","authority","evidence","reference","opened_at","matures_at","contest_until","state","space","acknowledged_at","approvals_required","departed""#;

impl PgRecoveryStore {
    /// Dial a connection of its own. Refuses when `$recovery` is absent.
    pub fn open(cfg: PoolConfig) -> Result<PgRecoveryStore, StoreFault> {
        PgRecoveryStore::start(Source::Dial(cfg))
    }

    /// On a connection somebody else owns. Never the request pool.
    pub fn open_on(pool: Pool) -> Result<PgRecoveryStore, StoreFault> {
        PgRecoveryStore::start(Source::Shared(pool))
    }

    fn start(source: Source) -> Result<PgRecoveryStore, StoreFault> {
        let db = PgSync::start(source, "recovery")?;
        db.sql(r#"SELECT 1 FROM "$recovery" LIMIT 1"#)?;
        Ok(PgRecoveryStore { db })
    }
}

fn case_of(r: &ordo_exec::TextRow) -> Option<Case> {
    let num = |i: usize| cell(r, i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    Some(Case {
        id: cell(r, 0)?,
        principal: cell(r, 1)?,
        path: Path::parse(&cell(r, 2)?)?,
        opened_by: cell(r, 3).unwrap_or_default(),
        authority: cell(r, 4).unwrap_or_default(),
        evidence: cell(r, 5).unwrap_or_default(),
        reference: cell(r, 6).unwrap_or_default(),
        opened_at: num(7),
        matures_at: num(8),
        contest_until: num(9),
        state: State::parse(&cell(r, 10)?)?,
        space: cell(r, 11).unwrap_or_default(),
        acknowledged: num(12) > 0,
        approvals_required: num(13).max(1) as u32,
        departed: cell(r, 14).unwrap_or_default(),
    })
}

fn states(list: &[State]) -> String {
    list.iter()
        .map(|s| sql_lit(s.as_str()))
        .collect::<Vec<_>>()
        .join(",")
}

impl RecoveryStore for PgRecoveryStore {
    fn insert(&self, c: &Case) -> Result<(), InsertError> {
        let rows = self
            .db
            .sql(&format!(
                r#"INSERT INTO "$recovery" ({CASE_COLUMNS})
                   VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, 0, {}, {})
                   ON CONFLICT DO NOTHING
                   RETURNING "id""#,
                sql_lit(&c.id),
                sql_lit(&c.principal),
                sql_lit(c.path.as_str()),
                sql_lit(&c.opened_by),
                sql_lit(&c.authority),
                sql_lit(&c.evidence),
                sql_lit(&c.reference),
                c.opened_at,
                c.matures_at,
                c.contest_until,
                sql_lit(c.state.as_str()),
                sql_lit(&c.space),
                c.approvals_required.max(1),
                sql_lit(&c.departed),
            ))
            .map_err(InsertError::Unavailable)?;
        if rows.is_empty() {
            return Err(InsertError::Live);
        }
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<Case>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT {CASE_COLUMNS} FROM "$recovery" WHERE "id" = {}"#,
            sql_lit(id)
        ))?;
        Ok(rows.first().and_then(case_of))
    }

    fn live_for(&self, principal: &str) -> Result<Option<Case>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT {CASE_COLUMNS} FROM "$recovery"
               WHERE "principal" = {} AND "state" IN ('pending','matured','completed')"#,
            sql_lit(principal)
        ))?;
        Ok(rows.first().and_then(case_of))
    }

    fn transition(
        &self,
        id: &str,
        from: &[State],
        to: State,
        contest_until: Option<u64>,
    ) -> Result<bool, StoreFault> {
        let also = match contest_until {
            Some(u) => format!(r#", "contest_until" = {u}"#),
            None => String::new(),
        };
        let rows = self.db.sql(&format!(
            r#"UPDATE "$recovery" SET "state" = {}{also}
               WHERE "id" = {} AND "state" IN ({})
               RETURNING "id""#,
            sql_lit(to.as_str()),
            sql_lit(id),
            states(from),
        ))?;
        Ok(!rows.is_empty())
    }

    fn close_expired(&self, now: u64) -> Result<Vec<Case>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"UPDATE "$recovery" SET "state" = 'closed'
               WHERE "state" = 'completed' AND "contest_until" <= {now}
               RETURNING {CASE_COLUMNS}"#
        ))?;
        Ok(rows.iter().filter_map(case_of).collect())
    }

    fn add_approval(&self, id: &str, approver: &str, at: u64) -> Result<bool, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"INSERT INTO "$recoveryapproval" ("case", "approver", "at")
               VALUES ({}, {}, {at})
               ON CONFLICT DO NOTHING
               RETURNING "case""#,
            sql_lit(id),
            sql_lit(approver)
        ))?;
        Ok(!rows.is_empty())
    }

    fn approvals(&self, id: &str) -> Result<Vec<String>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT "approver" FROM "$recoveryapproval" WHERE "case" = {} ORDER BY "at", "approver""#,
            sql_lit(id)
        ))?;
        Ok(rows.iter().filter_map(|r| cell(r, 0)).collect())
    }

    fn open_cases(&self) -> Result<Vec<Case>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT {CASE_COLUMNS} FROM "$recovery"
               WHERE "path" IN ('operator', 'succession') AND "state" IN ('pending', 'matured')
               ORDER BY "opened_at""#
        ))?;
        Ok(rows.iter().filter_map(case_of).collect())
    }

    fn add_objection(&self, o: &Objection) -> Result<bool, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"INSERT INTO "$recoveryobjection" ("case", "objector", "space", "at")
               VALUES ({}, {}, {}, {})
               ON CONFLICT DO NOTHING
               RETURNING "case""#,
            sql_lit(&o.case),
            sql_lit(&o.objector),
            sql_lit(&o.space),
            o.at
        ))?;
        Ok(!rows.is_empty())
    }

    fn objections(&self, case: &str) -> Result<Vec<Objection>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT "case", "objector", "space", "at", "resolved_at", "resolution", "reason",
                      "resolved_by"
               FROM "$recoveryobjection" WHERE "case" = {} ORDER BY "at", "objector""#,
            sql_lit(case)
        ))?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let num = |i: usize| cell(r, i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                Some(Objection {
                    case: cell(r, 0)?,
                    objector: cell(r, 1)?,
                    space: cell(r, 2).unwrap_or_default(),
                    at: num(3),
                    resolved_at: num(4),
                    resolution: cell(r, 5).unwrap_or_default(),
                    reason: cell(r, 6).unwrap_or_default(),
                    resolved_by: cell(r, 7).unwrap_or_default(),
                })
            })
            .collect())
    }

    fn resolve_objections(
        &self,
        case: &str,
        resolution: Resolution,
        reason: &str,
        by: &str,
        at: u64,
    ) -> Result<u32, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"UPDATE "$recoveryobjection"
               SET "resolved_at" = {}, "resolution" = {}, "reason" = {}, "resolved_by" = {}
               WHERE "case" = {} AND "resolved_at" = 0
               RETURNING "objector""#,
            at.max(1),
            sql_lit(resolution.as_str()),
            sql_lit(reason),
            sql_lit(by),
            sql_lit(case)
        ))?;
        Ok(rows.len() as u32)
    }

    fn unacknowledged(&self, principal: &str) -> Result<Vec<Case>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT {CASE_COLUMNS} FROM "$recovery"
               WHERE "principal" = {} AND "acknowledged_at" = 0 AND "path" <> 'succession'
                 AND "state" IN ('completed', 'closed')
               ORDER BY "opened_at""#,
            sql_lit(principal)
        ))?;
        Ok(rows.iter().filter_map(case_of).collect())
    }

    fn acknowledge(&self, id: &str, principal: &str, at: u64) -> Result<bool, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"UPDATE "$recovery" SET "acknowledged_at" = {}
               WHERE "id" = {} AND "principal" = {} AND "acknowledged_at" = 0
                 AND "path" <> 'succession' AND "state" IN ('completed', 'closed')
               RETURNING "id""#,
            at.max(1),
            sql_lit(id),
            sql_lit(principal)
        ))?;
        Ok(!rows.is_empty())
    }
}

// ── the service ──────────────────────────────────────────────────────────

/// Something that must happen once a recovery **completes** and not before:
/// removing every second factor the person had (owner, 2026-09-29: *"but only
/// after the reset"*). The second-factor plan supplies one. Until it does
/// there is none registered and nothing to remove.
pub trait Settlement: Send + Sync {
    fn name(&self) -> &'static str;
    fn settle(&self, principal: &str, case: &str) -> Result<(), StoreFault>;

    /// What a **contest** of `case` undoes here: everything of the person's
    /// enrolled at or after `opened`, the instant the case opened (owner
    /// ruling 2026-10-25). Returns each thing removed as `(kind, id)`, never
    /// a secret. The default undoes nothing.
    fn contest(
        &self,
        _principal: &str,
        _case: &str,
        _opened: u64,
    ) -> Result<Vec<(String, String)>, StoreFault> {
        Ok(Vec::new())
    }

    /// Called when a case opens, before it is stored: note what must be put
    /// back if the case is contested (owner ruling 2026-10-25). A failure
    /// stops the case opening, since a case without its note could not be
    /// undone.
    fn opened(&self, _principal: &str, _case: &str, _at: u64) -> Result<(), StoreFault> {
        Ok(())
    }

    /// What a **contest** puts back: each setting of the person's changed
    /// since the case opened goes back to its value when the case opened.
    /// Returns each reset, never a secret. The default resets nothing.
    fn reset(
        &self,
        _principal: &str,
        _case: &str,
        _opened: u64,
    ) -> Result<Vec<SettingReset>, StoreFault> {
        Ok(Vec::new())
    }
}

/// One setting a contest put back: what it is, the value it had when the
/// contest came (whoever held the recovered passkey set it) and the value it
/// has now (the one it had when the case opened). Words for a record, never a
/// secret and never an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingReset {
    pub kind: String,
    pub old: String,
    pub new: String,
}

/// The kind word for a directory link, for the record: the part of its id
/// before the first colon (`passkey`, `password`, an outside provider's).
fn link_kind(a: &AuthenticatorId) -> &str {
    a.0.split(':').next().unwrap_or("link")
}

/// What opening a case asks.
#[derive(Debug, Clone)]
pub struct Open<'a> {
    pub principal: &'a str,
    pub path: Path,
    pub opened_by: &'a str,
    pub authority: &'a str,
    pub evidence: &'a str,
    pub reference: &'a str,
    /// The tenant a succession is about. Empty for a recovery.
    pub space: &'a str,
    /// A delay the opener's own tenant has set, which can only lengthen the
    /// deployment's. Zero when there is none.
    pub extra_delay_secs: u64,
    /// The approvals the opener's own tenant has asked for, which can only
    /// raise the deployment's. Zero when there is none.
    pub approvals: u32,
}

/// What opening a case yields.
#[derive(Debug, Clone)]
pub struct Opened {
    pub case: String,
    pub matures_at: u64,
    /// The ticket, once, when the case matured at once. `None` when there is
    /// a delay: the opener collects it with [`Recovery::issue_ticket`] after
    /// the case matures.
    pub ticket: Option<String>,
    pub ticket_expires_at: Option<u64>,
    /// How many approvals the case needs before it yields a ticket. One for a
    /// case nobody else has to agree to.
    pub approvals_required: u32,
}

/// Why a case could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpenError {
    /// The person has a live case already.
    Live,
    /// The deployment has switched this path off.
    Disabled,
    /// The person signs in only through outside providers, who recover them.
    /// Ordo opens no case (owner, 2026-09-29: *"third party is managed
    /// elsewhere"*).
    OutsideProvider,
    Unavailable(StoreFault),
}

/// What is said to whoever tried to open a recovery for a person whose only
/// sign-ins belong to an outside provider.
pub const OUTSIDE_PROVIDER_SENTENCE: &str = "this person signs in through an outside provider, \
     which manages that sign-in. Recover it at the provider; Ordo does not";

/// Why a ticket could not be turned into a recovery. Always one refusal to
/// the caller, and a word for the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompleteError {
    Refused(&'static str),
    /// A store did not answer: the word for the stream, and whether it was
    /// busy (`crate::auth::StoreFailure`).
    Unavailable(&'static str, crate::auth::StoreFailure),
}

/// What a completed recovery did.
#[derive(Debug, Clone)]
pub struct Completed {
    pub principal: PrincipalId,
    pub case: String,
    pub added: AuthenticatorId,
    pub retired: Vec<AuthenticatorId>,
    pub presence_from: Timestamp,
    pub contest_until: Timestamp,
    /// Settlements that failed. The recovery stands and each is recorded.
    pub warnings: Vec<String>,
}

/// `-` for an empty field, so a detail line always has a value to read.
fn dash(s: &str) -> &str {
    if s.is_empty() {
        "-"
    } else {
        s
    }
}

/// The notice a sign-in carries for the recoveries a person has not
/// acknowledged: the case, the path, the class of authority and of evidence,
/// and where it stands. Never the deployment owner who acted, by name: a
/// person is told that the deployment acted, and the deployment's own stream
/// says who.
pub fn notices_json(cases: &[Case]) -> serde_json::Value {
    serde_json::Value::Array(
        cases
            .iter()
            .map(|c| {
                serde_json::json!({
                    "case": c.id,
                    "path": c.path.as_str(),
                    "authority": c.authority,
                    "evidence": c.evidence,
                    "state": c.state.as_str(),
                    "contest_until": c.contest_until,
                })
            })
            .collect(),
    )
}

/// Which tenants a person belongs to, by principal id.
pub type TenantsOf = Box<dyn Fn(&str) -> Vec<String> + Send + Sync>;

/// The tenants a person is an **active** member of, with the role held in
/// each (`owner`, `manager`, ...), by principal id. Who is told of a case
/// about a tenant's owner, and whose case a manager may object to.
pub type RolesOf = Box<dyn Fn(&str) -> Vec<(String, String)> + Send + Sync>;

/// [`RolesOf`] read from the door's engine: each tenant a person is an
/// active member of, with the role held there.
pub fn roles_from(door: Arc<crate::runtime::SwappableAccess>) -> RolesOf {
    Box::new(move |person| {
        door.current()
            .engine
            .spaces_of(&PrincipalId(person.to_string()))
            .into_iter()
            .filter(|(_, _, status)| *status == ordo_identity::MembershipStatus::Active)
            .filter_map(|(space, role, _)| role.map(|r| (space.0, r.as_str().to_string())))
            .collect()
    })
}

/// What completing a succession did to the departed owners' grants to apps
/// and their delegations in the tenant. The record names each one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ended {
    /// Whether the option was on for this completion.
    pub on: bool,
    /// Each grant ended, as `<grant id>@<app or ->`.
    pub grants: Vec<String>,
    /// Each delegation ended, as `<delegation id>@<agent>`.
    pub delegations: Vec<String>,
    /// Each one that could not be ended, with why, so the new owner knows.
    pub failed: Vec<String>,
}

/// A case a manager is told of: the case, the tenant it concerns them
/// through, and whether it is paused.
#[derive(Debug, Clone)]
pub struct ManagerNotice {
    pub case: Case,
    pub space: String,
    pub paused: bool,
    /// This manager has objected already.
    pub objected: bool,
}

/// The notice a sign-in carries for a tenant's manager: every open case
/// about an owner of a tenant they manage (a succession of it, or a
/// recovery through the door of one of its owners), with when it matures and
/// whether it is paused, until the case ends. A manager objects with
/// `std.space.objectToCase {case}` in that tenant.
pub fn manager_notices_json(notices: &[ManagerNotice]) -> serde_json::Value {
    serde_json::Value::Array(
        notices
            .iter()
            .map(|n| {
                serde_json::json!({
                    "case": n.case.id,
                    "tenant": n.space,
                    "path": n.case.path.as_str(),
                    "principal": n.case.principal,
                    "evidence": n.case.evidence,
                    "state": n.case.state.as_str(),
                    "matures_at": n.case.matures_at,
                    "departed": n
                        .case
                        .departed
                        .split(',')
                        .filter(|d| !d.is_empty())
                        .collect::<Vec<_>>(),
                    "paused": n.paused,
                    "objected": n.objected,
                    "object_with": "std.space.objectToCase",
                })
            })
            .collect(),
    )
}

fn joined(list: &[String]) -> String {
    if list.is_empty() {
        "-".to_string()
    } else {
        list.join(",")
    }
}

/// Cases, tickets, the directory's recover and contest, and the record.
pub struct Recovery {
    store: Arc<dyn RecoveryStore>,
    tickets: Arc<Tickets>,
    policy: RecoveryPolicy,
    stream: Option<Arc<Recorder>>,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
    tenants_of: TenantsOf,
    roles_of: RolesOf,
    settlements: Vec<Arc<dyn Settlement>>,
    codes: Option<Arc<crate::recovery_codes::RecoveryCodes>>,
    codes_gate: Option<Arc<dyn CodesGate>>,
    directory: Option<Arc<dyn ordo_identity::DirectoryStore>>,
}

/// Whether a person may use recovery codes at all: a tenant may switch codes
/// off for the people it created.
pub trait CodesGate: Send + Sync {
    fn allowed(&self, principal: &str) -> bool;
}

impl Recovery {
    pub fn new(
        store: Arc<dyn RecoveryStore>,
        tickets: Arc<Tickets>,
        policy: RecoveryPolicy,
    ) -> Recovery {
        Recovery {
            store,
            tickets,
            policy,
            stream: None,
            now: Box::new(crate::runtime::now_secs),
            tenants_of: Box::new(|_| Vec::new()),
            roles_of: Box::new(|_| Vec::new()),
            settlements: Vec::new(),
            codes: None,
            codes_gate: None,
            directory: None,
        }
    }

    /// Give recovery the directory it asks, before it opens a case, whether
    /// the person's sign-ins are all an outside provider's. Without one, no
    /// case is refused for that reason.
    pub fn with_directory(mut self, dir: Arc<dyn ordo_identity::DirectoryStore>) -> Recovery {
        self.directory = Some(dir);
        self
    }

    /// Give recovery the gate a tenant's choice about codes goes through.
    pub fn with_codes_gate(mut self, gate: Arc<dyn CodesGate>) -> Recovery {
        self.codes_gate = Some(gate);
        self
    }

    /// May this person issue or spend recovery codes? Yes unless a tenant that
    /// created them has switched codes off.
    pub fn codes_allowed(&self, principal: &str) -> bool {
        self.codes_gate
            .as_ref()
            .is_none_or(|g| g.allowed(principal))
    }

    /// Give recovery the person's codes. A recovery that completes voids the
    /// unused ones.
    pub fn with_codes(mut self, codes: Arc<crate::recovery_codes::RecoveryCodes>) -> Recovery {
        self.settlements
            .push(Arc::new(crate::recovery_codes::VoidCodes(codes.clone())));
        self.codes = Some(codes);
        self
    }

    /// The person's recovery codes, when the deployment has them.
    pub fn codes(&self) -> Option<&Arc<crate::recovery_codes::RecoveryCodes>> {
        self.codes.as_ref()
    }

    pub fn with_clock(mut self, now: Box<dyn Fn() -> u64 + Send + Sync>) -> Recovery {
        self.now = now;
        self
    }

    pub fn with_recorder(mut self, stream: Arc<Recorder>) -> Recovery {
        self.stream = Some(stream);
        self
    }

    /// Which tenants a person belongs to, so an act is mirrored into each
    /// one's own stream.
    pub fn with_tenants(mut self, tenants_of: TenantsOf) -> Recovery {
        self.tenants_of = tenants_of;
        self
    }

    /// The tenants a person is an active member of, with the role in each,
    /// so a case about a tenant's owner reaches that tenant's managers.
    pub fn with_roles(mut self, roles_of: RolesOf) -> Recovery {
        self.roles_of = roles_of;
        self
    }

    /// Register something to run when a recovery completes.
    pub fn with_settlement(mut self, s: Arc<dyn Settlement>) -> Recovery {
        self.settlements.push(s);
        self
    }

    pub fn policy(&self) -> &RecoveryPolicy {
        &self.policy
    }

    pub fn store(&self) -> &Arc<dyn RecoveryStore> {
        &self.store
    }

    fn now(&self) -> u64 {
        (self.now)()
    }

    /// The time recovery reads, for an answer that says whether a case has
    /// matured.
    pub fn clock(&self) -> u64 {
        self.now()
    }

    /// Record one act at deployment scope, and mirror it into every tenant
    /// the person belongs to. A mirrored entry names the person, the act and
    /// the class of authority, and never another tenant.
    pub fn record(&self, act: &str, subject: Subject, principal: &str, detail: String) {
        self.record_split(
            act,
            (subject.clone(), detail.clone()),
            (subject, detail),
            (self.tenants_of)(principal),
        );
    }

    /// Record one act with two faces: what the deployment's own stream says
    /// (who did it, by name) and what each tenant's stream says (the class of
    /// authority, never the person who held it). An operator's identity must
    /// not appear in a tenant's stream, and `tenants` is the tenants the act
    /// is mirrored into.
    pub fn record_split(
        &self,
        act: &str,
        deployment: (Subject, String),
        tenant: (Subject, String),
        tenants: Vec<String>,
    ) {
        let Some(stream) = &self.stream else {
            return;
        };
        let at = ordo_commandlog::Timestamp(self.now());
        let op = format!("recovery.{act}");
        let correlation = ordo_secevent::correlation::mint();
        let kind = |detail: &str| EventKind::Recovery {
            act: act.to_string(),
            detail: detail.to_string(),
        };
        stream.observe(
            Observation {
                space: None,
                subject: deployment.0,
                op: op.clone(),
                kind: kind(&deployment.1),
                at,
                correlation: correlation.clone(),
            },
            Relationship::None,
        );
        for space in tenants {
            stream.observe(
                Observation {
                    space: Some(ordo_commandlog::SpaceId(space)),
                    subject: tenant.0.clone(),
                    op: op.clone(),
                    kind: kind(&tenant.1),
                    at,
                    correlation: correlation.clone(),
                },
                Relationship::Member,
            );
        }
    }

    /// The subject the deployment's stream names for whoever opened a case.
    fn opener_subject(opened_by: &str, path: Path) -> Subject {
        match path {
            Path::Host => Subject::platform("cli"),
            _ => Subject::member(opened_by),
        }
    }

    /// The subject a tenant's stream names: the deployment owner is a class,
    /// not a person, so a tenant learns that the deployment acted and not who.
    fn tenant_subject(opened_by: &str, path: Path) -> Subject {
        match path {
            Path::Host | Path::Operator | Path::Succession => Subject::platform("deployment-owner"),
            Path::Code | Path::Tenant => Subject::member(opened_by),
        }
    }

    /// Open a case. When the path has no delay it matures at once and the
    /// ticket comes back in the answer, once. The caller has already checked
    /// that the person exists and that the opener had the authority.
    pub fn open(&self, req: &Open<'_>) -> Result<Opened, OpenError> {
        self.open_case(req, &[])
    }

    /// Open an owner succession, with the owners the deployment owner named
    /// as departed. Completing it ends their
    /// grants and delegations in the tenant when the option is on.
    pub fn open_succession(
        &self,
        req: &Open<'_>,
        departed: &[String],
    ) -> Result<Opened, OpenError> {
        debug_assert_eq!(req.path, Path::Succession);
        self.open_case(req, departed)
    }

    fn open_case(&self, req: &Open<'_>, departed: &[String]) -> Result<Opened, OpenError> {
        let now = self.now();
        // A completed case whose window has passed is closed here rather
        // than waiting for the sweep, so it cannot block a new one.
        let _ = self.store.close_expired(now);
        if !self.policy.enabled(req.path) {
            return Err(OpenError::Disabled);
        }
        // A succession changes who owns a tenant and touches no credential,
        // so it is not held back by where a person's sign-in comes from. Every
        // other path restores a way in, and that belongs to the provider.
        if req.path != Path::Succession {
            if let Some(dir) = &self.directory {
                // Fails closed: a directory that cannot be read opens nothing.
                let outside = ordo_identity::only_outside_provider(
                    dir.as_ref(),
                    &PrincipalId(req.principal.to_string()),
                )
                .map_err(|e| OpenError::Unavailable(e.fault()))?;
                if outside {
                    return Err(OpenError::OutsideProvider);
                }
            }
        }
        let delay = self.policy.delay_secs(req.path).max(req.extra_delay_secs);
        let approvals_required = self.policy.approvals_for(req.path).max(req.approvals);
        let matures_at = now.saturating_add(delay);
        let case = Case {
            id: format!("{CASE_PREFIX}{}", random_principal_suffix()),
            principal: req.principal.to_string(),
            path: req.path,
            opened_by: req.opened_by.to_string(),
            authority: req.authority.to_string(),
            evidence: req.evidence.to_string(),
            reference: req.reference.to_string(),
            opened_at: now,
            matures_at,
            contest_until: 0,
            state: if delay == 0 && approvals_required <= 1 {
                State::Matured
            } else {
                State::Pending
            },
            space: req.space.to_string(),
            acknowledged: false,
            approvals_required,
            departed: departed.join(","),
        };
        // Note the person's sign-in settings as they are now, so a contest
        // can put them back. Not for a succession: it touches no sign-in.
        if req.path != Path::Succession {
            for s in &self.settlements {
                s.opened(&case.principal, &case.id, now)
                    .map_err(OpenError::Unavailable)?;
            }
        }
        match self.store.insert(&case) {
            Ok(()) => {}
            Err(InsertError::Live) => return Err(OpenError::Live),
            Err(InsertError::Unavailable(e)) => return Err(OpenError::Unavailable(e)),
        }
        // The opener is the first approval, for the paths where a person
        // other than the subject opens a case that others may approve.
        if matches!(req.path, Path::Operator | Path::Succession | Path::Tenant) {
            let _ = self.store.add_approval(&case.id, req.opened_by, now);
        }
        let common = format!(
            "case={} principal={} path={} authority={} evidence={} checked={} ref={} matures-at={} space={} approvals-required={}",
            case.id,
            case.principal,
            req.path.as_str(),
            req.authority,
            req.evidence,
            // The platform verified a recovery code by redeeming it. It
            // cannot verify what a deployment owner says they saw, so the
            // class is recorded and the record says it was not checked.
            if req.path == Path::Code { "yes" } else { "no" },
            dash(req.reference),
            matures_at,
            dash(req.space),
            approvals_required,
        );
        let common = if req.path == Path::Succession {
            format!("{common} departed={}", joined(departed))
        } else {
            common
        };
        // A succession is mirrored into the tenant it is about and no other.
        let tenants = if req.path == Path::Succession {
            vec![req.space.to_string()]
        } else {
            (self.tenants_of)(req.principal)
        };
        self.record_split(
            "opened",
            (
                Recovery::opener_subject(req.opened_by, req.path),
                format!("{common} by={}", req.opened_by),
            ),
            (
                Recovery::tenant_subject(req.opened_by, req.path),
                format!(
                    "{common} by={}",
                    match req.path {
                        Path::Host | Path::Operator | Path::Succession => "deployment-owner",
                        _ => req.opened_by,
                    }
                ),
            ),
            tenants,
        );
        let (ticket, ticket_expires_at) =
            if case.state == State::Matured && req.path != Path::Succession {
                let issued = self.issue(&case).map_err(OpenError::Unavailable)?;
                (Some(issued.0), Some(issued.1))
            } else {
                (None, None)
            };
        Ok(Opened {
            case: case.id,
            matures_at,
            ticket,
            ticket_expires_at,
            approvals_required,
        })
    }

    fn issue(&self, case: &Case) -> Result<(String, u64), StoreFault> {
        let issued = self.tickets.issue(TicketSpec {
            purpose: Purpose::Recovery,
            principal: Some(PrincipalId(case.principal.clone())),
            display: String::new(),
            space: None,
            role: None,
            issued_by: case.opened_by.clone(),
            ttl_secs: self.policy.ticket_ttl_secs,
        })?;
        Ok((issued.ticket, issued.expires_at))
    }

    /// Collect the ticket of a case whose delay has passed. Once: the case
    /// moves from `pending` to `matured` in the same step, so a second call
    /// finds nothing to issue.
    pub fn issue_ticket(&self, case_id: &str) -> Result<(String, u64), CompleteError> {
        let now = self.now();
        let case = self
            .store
            .get(case_id)
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?
            .ok_or(CompleteError::Refused("unknown-case"))?;
        if case.path == Path::Succession || case.state != State::Pending || case.matures_at > now {
            return Err(CompleteError::Refused("case-not-mature"));
        }
        self.not_paused(&case.id)?;
        self.approved(&case)?;
        let moved = self
            .store
            .transition(case_id, &[State::Pending], State::Matured, None)
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?;
        if !moved {
            return Err(CompleteError::Refused("case-not-mature"));
        }
        self.issue(&case)
            .map_err(|e| CompleteError::Unavailable("ticket-store-unavailable", e.into()))
    }

    /// Has the case the approvals it needs? Refuses, naming the count, when
    /// not.
    fn approved(&self, case: &Case) -> Result<(), CompleteError> {
        // A recovery code and the host are proofs that need nobody else's
        // agreement: the person holds the paper, or whoever controls the
        // host is already sovereign. Only the paths a person other than the
        // subject opens are counted.
        if matches!(case.path, Path::Code | Path::Host) {
            return Ok(());
        }
        let have = self
            .store
            .approvals(&case.id)
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?
            .len() as u32;
        if have < case.approvals_required {
            return Err(CompleteError::Refused("case-not-approved"));
        }
        Ok(())
    }

    /// How many approvals a case has, and how many it needs.
    pub fn progress(&self, case_id: &str) -> Result<(u32, u32), StoreFault> {
        let case = self
            .store
            .get(case_id)?
            .ok_or_else(|| "no such case".to_string())?;
        Ok((
            self.store.approvals(case_id)?.len() as u32,
            case.approvals_required,
        ))
    }

    /// Approve a pending case of one of `paths`. Several people agreeing is
    /// the modifier that makes an assisted path harder to abuse: one persuaded
    /// person is not enough. **Neither the person being recovered nor somebody
    /// who has approved already may**, and each approval is recorded.
    pub fn approve(
        &self,
        case_id: &str,
        by: &str,
        paths: &[Path],
    ) -> Result<(u32, u32), CompleteError> {
        let case = self
            .store
            .get(case_id)
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?
            .filter(|c| paths.contains(&c.path))
            .ok_or(CompleteError::Refused("unknown-case"))?;
        if case.state != State::Pending {
            return Err(CompleteError::Refused("case-not-pending"));
        }
        if case.principal == by {
            return Err(CompleteError::Refused("approver-is-the-subject"));
        }
        let added = self
            .store
            .add_approval(case_id, by, self.now())
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?;
        if !added {
            return Err(CompleteError::Refused("already-approved"));
        }
        let have = self
            .store
            .approvals(case_id)
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?
            .len() as u32;
        let tenants = if case.path == Path::Tenant || case.path == Path::Succession {
            vec![case.space.clone()]
        } else {
            (self.tenants_of)(&case.principal)
        };
        let mirror = match case.path {
            Path::Tenant => Subject::member(by),
            _ => Subject::platform("deployment-owner"),
        };
        let common = format!(
            "case={} principal={} path={} approvals={have} of {}",
            case.id,
            case.principal,
            case.path.as_str(),
            case.approvals_required
        );
        self.record_split(
            "approved",
            (Subject::member(by), format!("{common} by={by}")),
            (
                mirror,
                format!(
                    "{common} by={}",
                    if case.path == Path::Tenant {
                        by
                    } else {
                        "deployment-owner"
                    }
                ),
            ),
            tenants,
        );
        Ok((have, case.approvals_required))
    }

    /// End a case that has not completed. `by` is who ended it and `why` a
    /// fixed word.
    pub fn cancel(&self, case_id: &str, by: Subject, why: &str) -> Result<bool, StoreFault> {
        let Some(case) = self.store.get(case_id)? else {
            return Ok(false);
        };
        let moved = self.store.transition(
            case_id,
            &[State::Pending, State::Matured],
            State::Cancelled,
            None,
        )?;
        if moved {
            self.record(
                "cancelled",
                by,
                &case.principal,
                format!("case={case_id} principal={} why={why}", case.principal),
            );
        }
        Ok(moved)
    }

    /// The person's live case and its state, for the notice a sign-in
    /// carries.
    pub fn live_for(&self, principal: &str) -> Result<Option<Case>, StoreFault> {
        self.store.live_for(principal)
    }

    /// A case by its id.
    pub fn case(&self, id: &str) -> Result<Option<Case>, StoreFault> {
        self.store.get(id)
    }

    /// The recoveries a person has not acknowledged, oldest first. What a
    /// sign-in carries until the person says they have seen it.
    pub fn notices(&self, principal: &str) -> Result<Vec<Case>, StoreFault> {
        self.store.unacknowledged(principal)
    }

    /// The person acknowledges a recovery, and the act is recorded. `false`
    /// when there is nothing of theirs to acknowledge.
    pub fn acknowledge(&self, case_id: &str, principal: &str) -> Result<bool, StoreFault> {
        let moved = self.store.acknowledge(case_id, principal, self.now())?;
        if moved {
            self.record(
                "acknowledged",
                Subject::member(principal),
                principal,
                format!("case={case_id} principal={principal}"),
            );
        }
        Ok(moved)
    }

    /// A succession that has come due: `pending` and past its delay becomes
    /// `matured`, and one already `matured` is returned as it is. Anything
    /// else, a recovery included, is refused. The caller then carries the
    /// succession out and calls [`Recovery::finish_succession`].
    pub fn mature_succession(&self, case_id: &str) -> Result<Case, CompleteError> {
        let now = self.now();
        let case = self
            .store
            .get(case_id)
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?
            .filter(|c| c.path == Path::Succession)
            .ok_or(CompleteError::Refused("unknown-case"))?;
        if matches!(case.state, State::Pending | State::Matured) {
            self.not_paused(&case.id)?;
        }
        match case.state {
            State::Matured => Ok(case),
            State::Pending if case.matures_at <= now => {
                self.approved(&case)?;
                self.store
                    .transition(case_id, &[State::Pending], State::Matured, None)
                    .map_err(|e| {
                        CompleteError::Unavailable("recovery-store-unavailable", e.into())
                    })?;
                Ok(Case {
                    state: State::Matured,
                    ..case
                })
            }
            _ => Err(CompleteError::Refused("case-not-mature")),
        }
    }

    /// A succession was carried out: the case is closed, once. Returns
    /// `false` when another request closed it first, and the caller then
    /// does nothing more. [`Recovery::record_succession`] records it.
    pub fn close_succession(&self, case: &Case) -> Result<bool, StoreFault> {
        self.store
            .transition(&case.id, &[State::Matured], State::Closed, None)
    }

    /// Record a succession that was carried out, in the deployment's stream
    /// and in the tenant's, naming each grant and delegation of the departed
    /// owners it ended.
    pub fn record_succession(&self, case: &Case, by: &str, ended: &Ended) {
        let common = format!(
            "case={} space={} to={} evidence={} ref={} departed={} ends-grants={} \
             grants-ended={} delegations-ended={} not-ended={}",
            case.id,
            case.space,
            case.principal,
            case.evidence,
            dash(&case.reference),
            dash(&case.departed),
            if ended.on { "on" } else { "off" },
            joined(&ended.grants),
            joined(&ended.delegations),
            joined(&ended.failed),
        );
        self.record_split(
            "owner-succeeded",
            (Subject::member(by), format!("{common} by={by}")),
            (
                Subject::platform("deployment-owner"),
                format!("{common} by=deployment-owner"),
            ),
            vec![case.space.clone()],
        );
    }

    // ── objections (owner, 2026-10-24) ─────────────────────────────────

    /// Refuse a case with an objection nobody has resolved.
    fn not_paused(&self, case_id: &str) -> Result<(), CompleteError> {
        if self
            .paused(case_id)
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?
        {
            return Err(CompleteError::Refused("case-paused"));
        }
        Ok(())
    }

    /// Has a case an objection the deployment owner has not resolved?
    pub fn paused(&self, case_id: &str) -> Result<bool, StoreFault> {
        Ok(self
            .store
            .objections(case_id)?
            .iter()
            .any(|o| o.resolved_at == 0))
    }

    /// Does this case concern the owner of `space`? A succession of that
    /// tenant does, and so does a recovery through the door of a person who
    /// owns it: the last-owner recovery the plan gives the same rule
    /// (`plan-passkey-recovery.md` section 6). Nothing else does.
    pub fn concerns(&self, case: &Case, space: &str) -> bool {
        match case.path {
            Path::Succession => case.space == space,
            Path::Operator => (self.roles_of)(&case.principal)
                .iter()
                .any(|(s, role)| s == space && role == "owner"),
            _ => false,
        }
    }

    /// The tenants a case is mirrored into: the tenant of a succession, every
    /// tenant of the person for a recovery.
    fn tenants_for(&self, case: &Case) -> Vec<String> {
        if case.path == Path::Tenant || case.path == Path::Succession {
            vec![case.space.clone()]
        } else {
            (self.tenants_of)(&case.principal)
        }
    }

    /// The open cases about an owner of each tenant `manager` manages: what
    /// their sign-in carries. A case about the manager themselves is left
    /// out.
    pub fn manager_notices(&self, manager: &str) -> Result<Vec<ManagerNotice>, StoreFault> {
        let managed: Vec<String> = (self.roles_of)(manager)
            .into_iter()
            .filter(|(_, role)| role == "manager")
            .map(|(space, _)| space)
            .collect();
        if managed.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for case in self.store.open_cases()? {
            if case.principal == manager {
                continue;
            }
            let Some(space) = managed.iter().find(|s| self.concerns(&case, s)) else {
                continue;
            };
            let objections = self.store.objections(&case.id)?;
            out.push(ManagerNotice {
                paused: objections.iter().any(|o| o.resolved_at == 0),
                objected: objections.iter().any(|o| o.objector == manager),
                space: space.clone(),
                case,
            });
        }
        Ok(out)
    }

    /// Every open case through the door or a succession, with its
    /// objections: what the console lists.
    pub fn open_cases(&self) -> Result<Vec<(Case, Vec<Objection>)>, StoreFault> {
        let mut out = Vec::new();
        for case in self.store.open_cases()? {
            let objections = self.store.objections(&case.id)?;
            out.push((case, objections));
        }
        Ok(out)
    }

    /// A manager of `space` objects to a case about its owner, which pauses
    /// the case. The caller has checked that `by` is a manager of `space`.
    /// Refused when the case is not open, does not concern `space`, is about
    /// `by`, or `by` has objected already. Recorded in the deployment's
    /// stream and in `space`'s, naming the manager: they are a member of that
    /// tenant, and the record goes to no other.
    pub fn object(&self, case_id: &str, by: &str, space: &str) -> Result<Case, CompleteError> {
        let case = self
            .store
            .get(case_id)
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?
            .filter(|c| matches!(c.path, Path::Operator | Path::Succession))
            .filter(|c| matches!(c.state, State::Pending | State::Matured))
            .filter(|c| self.concerns(c, space))
            .ok_or(CompleteError::Refused("not-an-open-case-here"))?;
        if case.principal == by {
            return Err(CompleteError::Refused("objector-is-the-subject"));
        }
        let now = self.now();
        let added = self
            .store
            .add_objection(&Objection {
                case: case.id.clone(),
                objector: by.to_string(),
                space: space.to_string(),
                at: now,
                resolved_at: 0,
                resolution: String::new(),
                reason: String::new(),
                resolved_by: String::new(),
            })
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?;
        if !added {
            return Err(CompleteError::Refused("already-objected"));
        }
        let detail = format!(
            "case={} principal={} path={} space={space} state=paused by={by}",
            case.id,
            case.principal,
            case.path.as_str(),
        );
        self.record_split(
            "objected",
            (Subject::member(by), detail.clone()),
            (Subject::member(by), detail),
            vec![space.to_string()],
        );
        Ok(case)
    }

    /// The deployment owner resolves every open objection to a case, with a
    /// reason from the decision's fixed list. **Resume** lets the case go on
    /// from where it was; **cancel** ends it. Refused when the case has no
    /// open objection. Answers how many objections were resolved and the
    /// case's state after.
    pub fn resolve(
        &self,
        case_id: &str,
        resolution: Resolution,
        reason: &str,
        by: &str,
    ) -> Result<(u32, State), CompleteError> {
        let unavailable =
            |e: StoreFault| CompleteError::Unavailable("recovery-store-unavailable", e.into());
        if !resolution.reasons().contains(&reason) {
            return Err(CompleteError::Refused("reason-not-on-the-list"));
        }
        let case = self
            .store
            .get(case_id)
            .map_err(unavailable)?
            .filter(|c| matches!(c.path, Path::Operator | Path::Succession))
            .ok_or(CompleteError::Refused("unknown-case"))?;
        if !matches!(case.state, State::Pending | State::Matured) {
            return Err(CompleteError::Refused("case-not-open"));
        }
        if !self.paused(case_id).map_err(unavailable)? {
            return Err(CompleteError::Refused("no-open-objection"));
        }
        let mut state = case.state;
        if resolution == Resolution::Cancel {
            if !self
                .store
                .transition(
                    case_id,
                    &[State::Pending, State::Matured],
                    State::Cancelled,
                    None,
                )
                .map_err(unavailable)?
            {
                return Err(CompleteError::Refused("case-not-open"));
            }
            state = State::Cancelled;
        }
        let n = self
            .store
            .resolve_objections(case_id, resolution, reason, by, self.now())
            .map_err(unavailable)?;
        let common = format!(
            "case={} principal={} path={} space={} decision={} reason={reason} objections={n} \
             state={}",
            case.id,
            case.principal,
            case.path.as_str(),
            dash(&case.space),
            resolution.as_str(),
            state.as_str(),
        );
        self.record_split(
            "objection-resolved",
            (Subject::member(by), format!("{common} by={by}")),
            (
                Subject::platform("deployment-owner"),
                format!("{common} by=deployment-owner"),
            ),
            self.tenants_for(&case),
        );
        Ok((n, state))
    }

    /// Look at a ticket without spending it: it must be a recovery ticket,
    /// unspent and unexpired, and its person's live case must be `matured`.
    /// The answer is that case. Anything else is one refusal.
    pub fn look(&self, ticket: &str) -> Result<Case, CompleteError> {
        let want = Want {
            purpose: Some(Purpose::Recovery),
            ..Default::default()
        };
        let peeked = self.tickets.peek(ticket, &want).map_err(|e| match e {
            // A ticket store that did not answer is not a refused ticket:
            // busy is `server_busy`, anything else the store's 503.
            e @ crate::tickets::TicketError::Unavailable(_) => {
                CompleteError::Unavailable("ticket-store-unavailable", e.into())
            }
            crate::tickets::TicketError::Refused => CompleteError::Refused("ticket-refused"),
        })?;
        let Some(principal) = peeked.principal else {
            return Err(CompleteError::Refused("ticket-names-nobody"));
        };
        self.store
            .live_for(&principal.0)
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?
            .filter(|c| c.state == State::Matured && c.path != Path::Succession)
            .ok_or(CompleteError::Refused("no-matured-case"))
            .and_then(|c| self.not_paused(&c.id).map(|()| c))
    }

    /// Turn a matured case's ticket into a recovery.
    ///
    /// The ticket is looked at without being spent, its person's live case
    /// must be `matured`, and the case is locked (`matured` to `completed`)
    /// before the directory is touched, so of two requests holding one ticket
    /// only one goes on. The directory recovers next and, if it refuses, the
    /// lock is taken back and **nothing has been spent**. Only then is the
    /// ticket spent, every session of the person ended, and the acts settled
    /// and recorded.
    pub fn complete(
        &self,
        dir: &mut Directory,
        revocations: &dyn SessionRevocations,
        session_max_secs: u64,
        ticket: &str,
        new: &AuthenticatorId,
        lost: &[AuthenticatorId],
    ) -> Result<Completed, CompleteError> {
        let now = self.now();
        let want = Want {
            purpose: Some(Purpose::Recovery),
            ..Default::default()
        };
        let peeked = self.tickets.peek(ticket, &want).map_err(|e| match e {
            // A ticket store that did not answer is not a refused ticket:
            // busy is `server_busy`, anything else the store's 503.
            e @ crate::tickets::TicketError::Unavailable(_) => {
                CompleteError::Unavailable("ticket-store-unavailable", e.into())
            }
            crate::tickets::TicketError::Refused => CompleteError::Refused("ticket-refused"),
        })?;
        let Some(principal) = peeked.principal.clone() else {
            return Err(CompleteError::Refused("ticket-names-nobody"));
        };
        let case = self
            .store
            .live_for(&principal.0)
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?
            .filter(|c| c.state == State::Matured && c.path != Path::Succession)
            .ok_or(CompleteError::Refused("no-matured-case"))?;
        self.not_paused(&case.id)?;
        let contest_until = now.saturating_add(self.policy.contest_secs);
        let locked = self
            .store
            .transition(
                &case.id,
                &[State::Matured],
                State::Completed,
                Some(contest_until),
            )
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?;
        if !locked {
            return Err(CompleteError::Refused("no-matured-case"));
        }
        let unlock = || {
            let _ = self
                .store
                .transition(&case.id, &[State::Completed], State::Matured, Some(0));
        };
        let rt = RecoveryTicket::new(
            principal.clone(),
            case.id.clone(),
            self.policy.quarantine_secs,
            self.policy.contest_secs,
        );
        let recovered = match dir.recover(&rt, new, lost, Timestamp(now)) {
            Ok(r) => r,
            Err(e) => {
                unlock();
                return Err(match e {
                    DirectoryError::StoreUnavailable(c) => {
                        CompleteError::Unavailable("directory-unavailable", c.into())
                    }
                    DirectoryError::AlreadyLinked { .. } => {
                        CompleteError::Refused("authenticator-already-linked")
                    }
                    _ => CompleteError::Refused("recovery-refused"),
                });
            }
        };
        if let Err(e) = self.tickets.redeem_for(ticket, &want) {
            // The ticket vanished between the look and the spend. Take the
            // directory's writes back rather than leave a recovery nobody
            // redeemed.
            let _ = dir.undo_recovery(&recovered);
            unlock();
            return Err(match e {
                e @ crate::tickets::TicketError::Unavailable(_) => {
                    CompleteError::Unavailable("ticket-store-unavailable", e.into())
                }
                _ => CompleteError::Refused("ticket-refused"),
            });
        }
        // Every session the person had ends, the recovering one included:
        // they sign in again with the passkey they just registered.
        let until = Timestamp(now.saturating_add(session_max_secs));
        if let Err(e) = revocations.end_all(&principal, Timestamp(now), until) {
            self.record(
                "sessions-not-ended",
                Subject::member(&principal.0),
                &principal.0,
                format!("case={}", case.id),
            );
            return Err(CompleteError::Unavailable(
                "session-store-unavailable",
                e.into(),
            ));
        }
        let mut warnings = Vec::new();
        for s in &self.settlements {
            if let Err(e) = s.settle(&principal.0, &case.id) {
                self.record(
                    "settlement-failed",
                    Subject::platform("recovery"),
                    &principal.0,
                    format!("case={} settlement={}", case.id, s.name()),
                );
                warnings.push(format!("{}: {e}", s.name()));
            }
        }
        self.record(
            "completed",
            Subject::member(&principal.0),
            &principal.0,
            format!(
                "case={} principal={} path={} authority={} added={} retired={} \
                 presence-from={} contest-until={}",
                case.id,
                principal.0,
                case.path.as_str(),
                case.authority,
                recovered.added.0,
                if recovered.retired.is_empty() {
                    "-".to_string()
                } else {
                    recovered
                        .retired
                        .iter()
                        .map(|a| a.0.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                },
                recovered.presence_from.0,
                recovered.contest_until.0,
            ),
        );
        Ok(Completed {
            principal,
            case: case.id,
            added: recovered.added,
            retired: recovered.retired,
            presence_from: recovered.presence_from,
            contest_until: recovered.contest_until,
            warnings,
        })
    }

    /// Contest a completed recovery with an authenticator it retired. The
    /// caller has verified an assertion from `by`. Removes what the recovery
    /// added, restores what it retired, ends every session and renewal
    /// again, voids every unused recovery code, and records who contested
    /// and how many codes went. The caller removes the public keys of
    /// `removed`.
    ///
    /// **Everything enrolled since the case opened goes too** (owner ruling
    /// 2026-10-25, board `what-a-contest-undoes-after-a`): every passkey or
    /// other sign-in linked at or after the case's `opened_at`, and every
    /// second factor (TOTP, security key) enrolled since, through each
    /// [`Settlement::contest`]. Otherwise the owner, back in with the
    /// restored passkey, could be asked for a step only the thief can give.
    /// Each removal is recorded (`contest-removed`, kind and id) in the
    /// person's stream and each tenant's. The owner always keeps `by`: it was
    /// made before the case and is restored.
    ///
    /// The second factors go first. If that fails the contest is refused as
    /// unavailable with nothing else changed, and `by` can contest again.
    ///
    /// **The codes go too** (audit F-1, 2026-10-24). Completing the recovery
    /// voided the codes from before it, so every unused code now was issued
    /// since, by whoever held the recovered passkey. Left alive, they let
    /// that person recover again after every contest.
    ///
    /// Refused when the case is no longer `completed`: one the deployment
    /// owner closed early ([`Recovery::close_contest`]) cannot be undone by
    /// an authenticator it retired.
    pub fn contest(
        &self,
        dir: &mut Directory,
        revocations: &dyn SessionRevocations,
        session_max_secs: u64,
        by: &AuthenticatorId,
    ) -> Result<Contested, CompleteError> {
        let now = self.now();
        let retired_by = dir
            .link_meta(by)
            .map_err(|e| match e {
                DirectoryError::StoreUnavailable(c) => {
                    CompleteError::Unavailable("directory-unavailable", c.into())
                }
                _ => CompleteError::Refused("nothing-to-contest"),
            })?
            .and_then(|m| m.retired)
            .ok_or(CompleteError::Refused("nothing-to-contest"))?;
        let case = self
            .store
            .get(&retired_by.case)
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?
            .filter(|c| c.state == State::Completed)
            .ok_or(CompleteError::Refused("contest-closed"))?;
        if retired_by.until <= now {
            return Err(CompleteError::Refused("nothing-to-contest"));
        }
        // What was enrolled since the case opened, outside the directory:
        // before anything else changes, so a failure leaves `by` able to
        // contest again.
        let mut factors_removed: Vec<(String, String)> = Vec::new();
        for s in &self.settlements {
            let gone = s
                .contest(&case.principal, &case.id, case.opened_at)
                .map_err(|e| {
                    CompleteError::Unavailable("second-factor-store-unavailable", e.into())
                })?;
            factors_removed.extend(gone);
        }
        // The settings changed since the case opened go back, in the same
        // pass and for the same reason: the sign-in the owner returns to
        // must be the one they left.
        let mut settings_reset: Vec<SettingReset> = Vec::new();
        for s in &self.settlements {
            let back = s
                .reset(&case.principal, &case.id, case.opened_at)
                .map_err(|e| CompleteError::Unavailable("settings-store-unavailable", e.into()))?;
            settings_reset.extend(back);
        }
        let contested = dir
            .contest(by, Timestamp(now), Timestamp(case.opened_at))
            .map_err(|e| match e {
                DirectoryError::StoreUnavailable(c) => {
                    CompleteError::Unavailable("directory-unavailable", c.into())
                }
                _ => CompleteError::Refused("nothing-to-contest"),
            })?;
        let _ = self
            .store
            .transition(&contested.case, &[State::Completed], State::Cancelled, None);
        let until = Timestamp(now.saturating_add(session_max_secs));
        revocations
            .end_all(&contested.principal, Timestamp(now), until)
            .map_err(|e| CompleteError::Unavailable("session-store-unavailable", e.into()))?;
        // The settlement completion runs, run again: every unused code goes.
        let voided = match &self.codes {
            None => "-".to_string(),
            Some(codes) => match crate::recovery_codes::VoidCodes(codes.clone())
                .settle(&contested.principal.0, &contested.case)
            {
                Ok(()) => "yes".to_string(),
                Err(_) => {
                    self.record(
                        "settlement-failed",
                        Subject::platform("recovery"),
                        &contested.principal.0,
                        format!("case={} settlement=recovery-codes", contested.case),
                    );
                    "failed".to_string()
                }
            },
        };
        // Each thing enrolled since the case opened, named by kind and id.
        let since = contested
            .enrolled_since
            .iter()
            .map(|a| (link_kind(a).to_string(), a.0.clone()))
            .chain(factors_removed);
        let mut since_named = Vec::new();
        for (kind, id) in since {
            self.record(
                "contest-removed",
                Subject::member(&contested.principal.0),
                &contested.principal.0,
                format!(
                    "case={} principal={} kind={kind} id={id} enrolled-since={}",
                    contested.case, contested.principal.0, case.opened_at
                ),
            );
            since_named.push(format!("{kind}/{id}"));
        }
        let mut reset_named = Vec::new();
        for r in settings_reset {
            self.record(
                "contest-reset",
                Subject::member(&contested.principal.0),
                &contested.principal.0,
                format!(
                    "case={} principal={} kind={} old={} new={}",
                    contested.case, contested.principal.0, r.kind, r.old, r.new
                ),
            );
            reset_named.push(r.kind);
        }
        let reset_named = if reset_named.is_empty() {
            "-".to_string()
        } else {
            reset_named.join(",")
        };
        let since_named = if since_named.is_empty() {
            "-".to_string()
        } else {
            since_named.join(",")
        };
        self.record(
            "contested",
            Subject::member(&contested.principal.0),
            &contested.principal.0,
            format!(
                "case={} principal={} by={} codes-voided={voided} removed={} restored={} \
                 enrolled-since-removed={since_named} settings-reset={reset_named}",
                contested.case,
                contested.principal.0,
                by.0,
                contested
                    .removed
                    .iter()
                    .map(|a| a.0.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                contested
                    .restored
                    .iter()
                    .map(|a| a.0.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        );
        Ok(contested)
    }

    /// **The deployment owner closes a recovery's contest window early**
    /// (`plan-passkey-recovery.md` §4.2), with a reason from
    /// [`CLOSE_CONTEST_REASONS`]. The case goes from `completed` to `closed`
    /// at once, so:
    ///
    /// - nothing the case retired can contest it any more (each retired
    ///   link's window is ended now, and the sweep removes it and its key);
    /// - the person may have a new case opened straight away, where until now
    ///   they waited out the window;
    /// - a passkey the case added still may not cancel a later case until
    ///   the time the window was given has passed, so closing a thief's case
    ///   to open the owner's does not hand the thief a veto.
    ///
    /// Recorded in the deployment's stream, naming `by`, and in each tenant
    /// the person belongs to as the deployment owner. Refused when the case
    /// is not a completed recovery.
    pub fn close_contest(
        &self,
        directory: &dyn ordo_identity::DirectoryStore,
        case_id: &str,
        reason: &str,
        by: &str,
    ) -> Result<ContestClosed, CompleteError> {
        let unavailable =
            |e: StoreFault| CompleteError::Unavailable("recovery-store-unavailable", e.into());
        if !CLOSE_CONTEST_REASONS.contains(&reason) {
            return Err(CompleteError::Refused("reason-not-on-the-list"));
        }
        let case = self
            .store
            .get(case_id)
            .map_err(unavailable)?
            .filter(|c| c.path != Path::Succession)
            .ok_or(CompleteError::Refused("unknown-case"))?;
        if case.state != State::Completed {
            return Err(CompleteError::Refused("case-not-in-contest"));
        }
        let now = self.now();
        if !self
            .store
            .transition(case_id, &[State::Completed], State::Closed, None)
            .map_err(unavailable)?
        {
            return Err(CompleteError::Refused("case-not-in-contest"));
        }
        // The state is the lock: `contest` refuses a case that is not
        // completed. Ending each retired link's window as well lets the
        // sweep remove the links and their keys now, not in three days.
        let mut retired = Vec::new();
        let links = directory
            .links_of(&PrincipalId(case.principal.clone()))
            .map_err(|e| CompleteError::Unavailable("directory-unavailable", e.into()))?;
        for (a, meta) in links {
            if meta.retired.as_ref().is_some_and(|r| r.case == case.id) {
                directory
                    .set_retired(
                        &a,
                        Some(&ordo_identity::directory::Retired {
                            case: case.id.clone(),
                            until: now,
                        }),
                    )
                    .map_err(|e| CompleteError::Unavailable("directory-unavailable", e.into()))?;
                retired.push(a);
            }
        }
        let common = format!(
            "case={} principal={} path={} reason={reason} window-was-until={} retired={}",
            case.id,
            case.principal,
            case.path.as_str(),
            case.contest_until,
            if retired.is_empty() {
                "-".to_string()
            } else {
                retired
                    .iter()
                    .map(|a| a.0.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            },
        );
        self.record_split(
            "contest-closed",
            (Subject::member(by), format!("{common} by={by}")),
            (
                Subject::platform("deployment-owner"),
                format!("{common} by=deployment-owner"),
            ),
            self.tenants_for(&case),
        );
        Ok(ContestClosed {
            case: case.id,
            principal: case.principal,
            retired,
        })
    }

    /// Close the cases whose contest window ended and remove the retired
    /// links. Returns the links removed, so the caller can remove the public
    /// keys behind them.
    pub fn sweep(
        &self,
        dir: &mut Directory,
    ) -> Result<Vec<(AuthenticatorId, PrincipalId, String)>, StoreFault> {
        let now = self.now();
        let gone = dir
            .purge_retired(Timestamp(now))
            .map_err(|e| e.to_string())?;
        for c in self.store.close_expired(now)? {
            self.record(
                "closed",
                Subject::platform("recovery"),
                &c.principal,
                format!("case={} principal={}", c.id, c.principal),
            );
        }
        Ok(gone)
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    fn with(pairs: &[(&str, &str)]) -> Result<RecoveryPolicy, PolicyError> {
        let map: std::collections::BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        RecoveryPolicy::from_lookup(&|k| map.get(k).cloned())
    }

    #[test]
    fn nothing_set_is_the_safe_defaults() {
        assert_eq!(with(&[]).expect("defaults"), RecoveryPolicy::default());
        let d = RecoveryPolicy::default();
        assert_eq!(d.quarantine_secs, 86_400, "a day (owner, 2026-09-29)");
        assert_eq!(d.code_delay_secs, 0, "a code works at once (owner)");
        assert!(
            !d.hold_default,
            "a recovered person is not locked out (owner)"
        );
        assert!(d.paths.contains(&Path::Host) && d.paths.contains(&Path::Code));
    }

    #[test]
    fn a_setting_is_adjustable_and_a_floor_is_a_refusal_not_a_clamp() {
        let p = with(&[
            ("ORDO_RECOVERY_QUARANTINE_SECS", "7200"),
            ("ORDO_RECOVERY_CONTEST_SECS", "7200"),
            ("ORDO_RECOVERY_OPERATOR_APPROVALS", "3"),
            ("ORDO_RECOVERY_PATHS", "code, operator"),
            ("ORDO_RECOVERY_HOLD_DEFAULT", "on"),
        ])
        .expect("adjusted");
        assert_eq!(p.quarantine_secs, 7_200);
        assert_eq!(p.operator_approvals, 3);
        assert_eq!(p.paths, vec![Path::Code, Path::Operator]);
        assert!(p.hold_default);
        assert!(!RecoveryPolicy::default().managers_may_recover);
        assert!(
            with(&[("ORDO_RECOVERY_MANAGERS", "on")])
                .expect("on")
                .managers_may_recover
        );
        assert!(!p.enabled(Path::Host) && !p.enabled(Path::Tenant));
        assert!(p.enabled(Path::Succession), "a succession follows the door");
        let e = with(&[("ORDO_RECOVERY_QUARANTINE_SECS", "3599")]).expect_err("below the floor");
        assert_eq!(e.var, "ORDO_RECOVERY_QUARANTINE_SECS");
        assert!(e.detail.contains("floor of 3600"), "{}", e.detail);
    }

    #[test]
    fn a_quarantine_longer_than_the_window_is_refused_by_name() {
        let e = with(&[
            ("ORDO_RECOVERY_QUARANTINE_SECS", "90000"),
            ("ORDO_RECOVERY_CONTEST_SECS", "86400"),
        ])
        .expect_err("longer than the window");
        assert_eq!(e.var, "ORDO_RECOVERY_QUARANTINE_SECS");
        assert!(
            e.detail.contains("ORDO_RECOVERY_CONTEST_SECS"),
            "{}",
            e.detail
        );
    }

    #[test]
    fn the_rest_refuse_by_name() {
        for (var, value) in [
            ("ORDO_RECOVERY_HOST_DELAY_SECS", "-1"),
            ("ORDO_RECOVERY_PATHS", "code,,fax"),
            ("ORDO_RECOVERY_TENANT_APPROVALS", "0"),
            ("ORDO_RECOVERY_OPERATOR_APPROVALS", "many"),
            ("ORDO_RECOVERY_HOLD_DEFAULT", "1"),
            ("ORDO_RECOVERY_MANAGERS", "yes"),
        ] {
            let e = with(&[(var, value)]).expect_err(var);
            assert_eq!(e.var, var, "{value}");
        }
    }

    #[test]
    fn a_succession_ends_the_departed_owners_grants_unless_set_off() {
        assert!(
            RecoveryPolicy::default().succession_ends_grants,
            "on by default (owner, 2026-10-24)"
        );
        let off = with(&[("ORDO_SUCCESSION_ENDS_GRANTS", "off")]).expect("off");
        assert!(!off.succession_ends_grants);
        assert!(off.describe().contains("succession ends grants off"));
        let e = with(&[("ORDO_SUCCESSION_ENDS_GRANTS", "no")]).expect_err("not a word");
        assert_eq!(e.var, "ORDO_SUCCESSION_ENDS_GRANTS");
        assert!(e.detail.contains("default on"), "{}", e.detail);
    }

    #[test]
    fn the_startup_line_says_what_was_settled() {
        let line = RecoveryPolicy::default().describe();
        for want in [
            "quarantine 24h",
            "contest 72h",
            "door 24h",
            "hold by default off",
        ] {
            assert!(line.contains(want), "{want} in {line}");
        }
    }

    #[test]
    fn ticket_life_and_codes_per_set_are_settings_with_bounds() {
        let d = RecoveryPolicy::default();
        assert_eq!(d.ticket_ttl_secs, 3 * 24 * 3600);
        assert_eq!(d.codes_per_set, 10);
        let set = RecoveryPolicy::from_lookup(&|k| match k {
            "ORDO_RECOVERY_TICKET_SECS" => Some("7200".into()),
            "ORDO_RECOVERY_CODES_PER_SET" => Some("12".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!((set.ticket_ttl_secs, set.codes_per_set), (7200, 12));
        for (var, bad) in [
            ("ORDO_RECOVERY_TICKET_SECS", "60"),
            ("ORDO_RECOVERY_TICKET_SECS", "99999999"),
            ("ORDO_RECOVERY_CODES_PER_SET", "0"),
            ("ORDO_RECOVERY_CODES_PER_SET", "51"),
            ("ORDO_RECOVERY_CODES_PER_SET", "ten"),
        ] {
            let e = RecoveryPolicy::from_lookup(&|k| (k == var).then(|| bad.to_string()))
                .expect_err(bad);
            assert_eq!(e.var, var);
            assert!(e.detail.contains("default"), "{}", e.detail);
        }
    }
}
