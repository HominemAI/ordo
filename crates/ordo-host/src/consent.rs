//! The consent ceremony: a person lets an agent act for them, through the
//! door (checklist 11-4b, `plan-delegation-surface.md` increments 3 and 4).
//!
//! # The five steps
//!
//! 1. **The agent asks.** `POST /auth/delegation/request` names the tenant,
//!    the agent, the person and the ops. The agent holds no token yet, so the
//!    route is open. It answers a request id and a **binder**. Only the
//!    binder's hash is stored. The binder never goes in a URL.
//! 2. **The person reads.** `std.delegation.review($request)` answers the
//!    disclosure. Every word on it is computed from the deployment: the ops'
//!    field-scope manifests, the app developer's name for the agent, the
//!    term as a date. Nothing the agent sent reaches it as prose.
//! 3. **The person says yes, with presence.** `std.delegation.approve` is a
//!    platform op, so the door's step-up gate stands in front of it: a fresh
//!    passkey touch, or a two-step claim the deployment accepts. The app
//!    developer may exempt it with `ORDO_STEP_UP_EXEMPT_OPS`, as for every
//!    platform op. A delegated or capability token never satisfies it, and
//!    platform writes are closed to delegated tokens besides, so an agent
//!    cannot grant a delegation, to itself or onward.
//! 4. **Or no.** `std.delegation.decline`. Recorded, like everything here.
//! 5. **The agent redeems.** `POST /auth/delegation/redeem` with the request
//!    id **and the binder**. A request id seen on a screen or in a log is
//!    worth nothing without the binder, which only the agent held. The
//!    redeem is one-shot: the row moves to `redeemed` in the statement that
//!    reads it.
//!
//! The person lists their delegations with `std.delegation.mine` and ends
//! one with `std.delegation.revoke`. The door refuses the agent's next call.
//!
//! # Who the agent is
//!
//! An agent has no credential of its own yet (worker credentials come after
//! the first deployment, owner 2026-10-08). So the deployment names the
//! agents it accepts requests for, with the name the person sees:
//! `ORDO_DELEGATION_AGENTS=agt_assistant=Expense assistant`. The name on the
//! review page is the app developer's, never the agent's. Unset, no request
//! is accepted, and the refusal names the setting.
//!
//! # What is granted
//!
//! Exactly the ops the person saw marked as granted: ops of the program,
//! open to delegation (`ORDO_DELEGABLE_OPS` for deletes, never a platform
//! op), and asking for no presence (presence is not delegable). Each is
//! pinned in a capability grant minted in the same act as the delegation.
//! If either record fails, neither stands and no token is issued.
//!
//! # The record
//!
//! Each step is a security event: `delegation.requested`, `.approved`,
//! `.declined`, `.refused` (with the reason), `.redeemed`, `.revoked`. The
//! approval's correlation id is the delegation's `consent_correlation`.
//! **A departure from the plan**: R4 asks for a command group in the
//! tenant's chain. A consent writes no tenant row, so it is recorded in the
//! security stream, as the membership commands record theirs.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_conformance::Fixture;
use ordo_exec::Pool;
use ordo_identity::delegation::{DelegationRecord, DelegationRegistry, NewDelegation};
use ordo_identity::grants::{GrantRegistry, NewGrant};
use ordo_identity::principal::{
    CapabilityGrant, DelegationId, GrantId, PrincipalId, Role, SpaceId, Timestamp,
};
use ordo_identity::{AccessEngine, TokenSpec};
use ordo_secevent::{EventKind, Observation, Recorder, Relationship, Subject};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

// ── the ops ──────────────────────────────────────────────────────────────

/// The disclosure, for the person the request names.
pub const REVIEW: &str = "std.delegation.review";
/// Yes. Asks for presence unless the app developer exempted it.
pub const APPROVE: &str = "std.delegation.approve";
/// No.
pub const DECLINE: &str = "std.delegation.decline";
/// The caller's requests and delegations in this tenant.
pub const MINE: &str = "std.delegation.mine";
/// End one of the caller's delegations.
pub const REVOKE: &str = "std.delegation.revoke";

/// Every op of this module, for registration.
pub const OPS: [&str; 5] = [REVIEW, APPROVE, DECLINE, MINE, REVOKE];

/// Every role may call them: a person lends their own rights, whatever
/// their role, and the delegation never exceeds that role.
pub fn any_member(name: &str) -> bool {
    OPS.contains(&name)
}

/// Only the approval asks for presence. Reading, declining, listing and
/// revoking take nothing away from anyone but the agent.
pub fn asks_presence(name: &str) -> bool {
    name == APPROVE
}

/// The specs, registered with the other platform ops.
pub fn specs() -> Vec<OpSpec> {
    let text = |name: &str| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(64) },
        required: true,
        bounds: None,
    };
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>, out: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        sync: if kind == OpKind::Read {
            Syncability::NotSyncable(NotSyncable::NoAnchor)
        } else {
            Syncability::NotSyncable(NotSyncable::NotARead)
        },
        kind,
        params,
        output_fields: out.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        spec(
            REVIEW,
            OpKind::Read,
            vec![text("request")],
            &[
                "request",
                "tenant",
                "agent",
                "person",
                "term",
                "requestExpiresAt",
                "ops",
                "notGranted",
                "unknownNames",
                "cannot",
                "ceiling",
                "revoke",
                "ownerSees",
            ],
        ),
        spec(
            APPROVE,
            OpKind::Write,
            vec![text("request")],
            &[
                "request",
                "delegation",
                "agent",
                "ops",
                "expiresAt",
                "replaced",
                "outcome",
            ],
        ),
        spec(
            DECLINE,
            OpKind::Write,
            vec![text("request")],
            &["request", "outcome"],
        ),
        spec(
            MINE,
            OpKind::Read,
            Vec::new(),
            &[
                "request",
                "delegation",
                "agent",
                "agentName",
                "state",
                "ops",
                "grantedAt",
                "expiresAt",
                "revokedAt",
            ],
        ),
        spec(
            REVOKE,
            OpKind::Write,
            vec![text("delegation")],
            &["delegation", "outcome"],
        ),
    ]
}

// ── the settings ─────────────────────────────────────────────────────────

/// The agents the deployment accepts requests for, each with the name the
/// person sees: `agt_assistant=Expense assistant,agt_mail=Mail sorter`.
pub const AGENTS_ENV: &str = "ORDO_DELEGATION_AGENTS";
/// The longest delegation a person may grant, in hours.
pub const MAX_HOURS_ENV: &str = "ORDO_DELEGATION_MAX_HOURS";
/// How long a request waits for the person, and then for the agent to
/// redeem it, in seconds.
pub const REQUEST_SECS_ENV: &str = "ORDO_DELEGATION_REQUEST_SECS";
/// The most ops one request may name.
pub const MAX_OPS_ENV: &str = "ORDO_DELEGATION_MAX_OPS";
/// The most requests one agent may have waiting at once, across every
/// tenant and person.
pub const OPEN_PER_AGENT_ENV: &str = "ORDO_DELEGATION_OPEN_PER_AGENT";
/// The most requests one person may have waiting at once, from every agent.
pub const OPEN_PER_PERSON_ENV: &str = "ORDO_DELEGATION_OPEN_PER_PERSON";

pub const DEFAULT_MAX_HOURS: u64 = 8;
pub const DEFAULT_REQUEST_SECS: u64 = 600;
pub const DEFAULT_MAX_OPS: usize = 10;
pub const DEFAULT_OPEN_PER_AGENT: u64 = 1000;
pub const DEFAULT_OPEN_PER_PERSON: u64 = 10;
/// The ranges a boot accepts.
const MAX_HOURS_RANGE: (u64, u64) = (1, 720);
const REQUEST_SECS_RANGE: (u64, u64) = (60, 3600);
const MAX_OPS_RANGE: (u64, u64) = (1, 100);
const OPEN_PER_AGENT_RANGE: (u64, u64) = (1, 1_000_000);
const OPEN_PER_PERSON_RANGE: (u64, u64) = (1, 1000);
/// The term when a request names none.
pub const DEFAULT_HOURS: u64 = 1;

/// The ceremony's settings. The defaults are the safe ones: no agent is
/// accepted, a delegation lasts at most 8 hours, a request waits 10
/// minutes, and names at most 10 ops. One agent may have 1000 requests
/// waiting, and one person 10.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Agent id to the name the person sees.
    pub agents: BTreeMap<String, String>,
    pub max_hours: u64,
    pub request_secs: u64,
    pub max_ops: usize,
    /// Waiting requests one agent may have at once.
    pub open_per_agent: u64,
    /// Waiting requests one person may have at once.
    pub open_per_person: u64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            agents: BTreeMap::new(),
            max_hours: DEFAULT_MAX_HOURS,
            request_secs: DEFAULT_REQUEST_SECS,
            max_ops: DEFAULT_MAX_OPS,
            open_per_agent: DEFAULT_OPEN_PER_AGENT,
            open_per_person: DEFAULT_OPEN_PER_PERSON,
        }
    }
}

fn agent_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

fn number(
    var: &'static str,
    v: &str,
    (lo, hi): (u64, u64),
    default: u64,
) -> Result<u64, (&'static str, String)> {
    match v.trim().parse::<u64>() {
        Ok(n) if (lo..=hi).contains(&n) => Ok(n),
        _ => Err((
            var,
            format!("`{v}` is not a whole number from {lo} to {hi}; unset it for the default, {default}"),
        )),
    }
}

impl Settings {
    /// Parse the agent list. Each entry is `id` or `id=name`.
    pub fn parse_agents(list: &str) -> Result<BTreeMap<String, String>, String> {
        let mut out = BTreeMap::new();
        for entry in list.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let (id, name) = match entry.split_once('=') {
                Some((id, name)) => (id.trim(), name.trim()),
                None => (entry, entry),
            };
            if !agent_id_ok(id) {
                return Err(format!(
                    "`{id}`: an agent id is letters, digits, `_`, `-` and `.`, at most 64"
                ));
            }
            if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
                return Err(format!(
                    "`{id}`: the name is 1 to 80 characters with no control characters"
                ));
            }
            if out.insert(id.to_string(), name.to_string()).is_some() {
                return Err(format!("`{id}` is named twice"));
            }
        }
        Ok(out)
    }

    /// Read the six settings. Unset is the default; a value that cannot
    /// be used refuses the boot by name, with its range and its default.
    pub fn from_env() -> Result<Settings, (&'static str, String)> {
        Settings::from_vars(&|k| std::env::var(k).ok())
    }

    /// [`Settings::from_env`] over any source of variables.
    pub fn from_vars(
        var: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Settings, (&'static str, String)> {
        let mut s = Settings::default();
        if let Some(v) = var(AGENTS_ENV) {
            s.agents = Settings::parse_agents(&v).map_err(|e| (AGENTS_ENV, e))?;
        }
        if let Some(v) = var(MAX_HOURS_ENV) {
            s.max_hours = number(MAX_HOURS_ENV, &v, MAX_HOURS_RANGE, DEFAULT_MAX_HOURS)?;
        }
        if let Some(v) = var(REQUEST_SECS_ENV) {
            s.request_secs = number(
                REQUEST_SECS_ENV,
                &v,
                REQUEST_SECS_RANGE,
                DEFAULT_REQUEST_SECS,
            )?;
        }
        if let Some(v) = var(MAX_OPS_ENV) {
            s.max_ops = number(MAX_OPS_ENV, &v, MAX_OPS_RANGE, DEFAULT_MAX_OPS as u64)? as usize;
        }
        if let Some(v) = var(OPEN_PER_AGENT_ENV) {
            s.open_per_agent = number(
                OPEN_PER_AGENT_ENV,
                &v,
                OPEN_PER_AGENT_RANGE,
                DEFAULT_OPEN_PER_AGENT,
            )?;
        }
        if let Some(v) = var(OPEN_PER_PERSON_ENV) {
            s.open_per_person = number(
                OPEN_PER_PERSON_ENV,
                &v,
                OPEN_PER_PERSON_RANGE,
                DEFAULT_OPEN_PER_PERSON,
            )?;
        }
        Ok(s)
    }

    /// One line for the boot census.
    pub fn describe(&self) -> String {
        if self.agents.is_empty() {
            format!("no agent may ask ({AGENTS_ENV} unset)")
        } else {
            format!(
                "agents {}; at most {}h, {} ops; a request waits {}s; at most {} waiting per \
                 agent and {} per person",
                self.agents.keys().cloned().collect::<Vec<_>>().join(", "),
                self.max_hours,
                self.max_ops,
                self.request_secs,
                self.open_per_agent,
                self.open_per_person
            )
        }
    }
}

// ── the table ────────────────────────────────────────────────────────────

/// One row per request, in the deployment's own database beside
/// `"$delegation"`. The binder is stored as its SHA-256.
pub const REQUEST_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS "$delegation_request" (
    "request_id"         text   PRIMARY KEY,
    "$space"             text   NOT NULL,
    "agent"              text   NOT NULL,
    "person"             text   NOT NULL,
    "ops"                text   NOT NULL,
    "term_secs"          bigint NOT NULL,
    "binder"             text   NOT NULL,
    "created_at"         bigint NOT NULL,
    "expires_at"         bigint NOT NULL,
    "status"             text   NOT NULL,
    "decided_at"         bigint,
    "delegator"          text,
    "delegation_id"      text,
    "grant_id"           text,
    "granted_ops"        text,
    "delegation_expires" bigint
)"#;

pub const REQUEST_INDEX_DDL: &str = r#"
CREATE INDEX IF NOT EXISTS "$delegation_request$person"
    ON "$delegation_request" ("$space", "person")"#;

/// For counting one agent's waiting requests (`ORDO_DELEGATION_OPEN_PER_AGENT`).
pub const REQUEST_AGENT_INDEX_DDL: &str = r#"
CREATE INDEX IF NOT EXISTS "$delegation_request$agent"
    ON "$delegation_request" ("agent", "status", "expires_at")"#;

fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn sha256_hex(s: &str) -> String {
    ordo_crypto::hex(&Sha256::digest(s.as_bytes()))
}

fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    ordo_crypto::fill_random(&mut b);
    ordo_crypto::hex(&b)
}

const COLUMNS: &str = r#""request_id", "$space", "agent", "person", "ops", "term_secs", "created_at", "expires_at", "status", "decided_at", "delegator", "delegation_id", "grant_id", "granted_ops", "delegation_expires""#;

/// One request, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRow {
    pub id: String,
    pub space: String,
    pub agent: String,
    /// The member id of the person asked.
    pub person: String,
    pub ops: Vec<String>,
    pub term_secs: u64,
    pub created_at: u64,
    pub expires_at: u64,
    /// `waiting`, `approved`, `declined`, `redeemed` or `failed`.
    pub status: String,
    pub decided_at: Option<u64>,
    pub delegator: Option<String>,
    pub delegation: Option<String>,
    pub grant: Option<String>,
    pub granted: Vec<String>,
    pub delegation_expires: Option<u64>,
}

fn split_ops(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn row_of(r: &[Option<String>]) -> Option<RequestRow> {
    let s = |i: usize| r.get(i).cloned().flatten();
    let n = |i: usize| s(i).and_then(|v| v.parse::<u64>().ok());
    Some(RequestRow {
        id: s(0)?,
        space: s(1)?,
        agent: s(2)?,
        person: s(3)?,
        ops: split_ops(&s(4)?),
        term_secs: n(5)?,
        created_at: n(6)?,
        expires_at: n(7)?,
        status: s(8)?,
        decided_at: n(9),
        delegator: s(10),
        delegation: s(11),
        grant: s(12),
        granted: s(13).map(|g| split_ops(&g)).unwrap_or_default(),
        delegation_expires: n(14),
    })
}

// ── what the agent sends and gets ────────────────────────────────────────

/// The agent's request, as the route read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRequest {
    pub space: String,
    pub agent: String,
    /// The member id of the person who will be asked.
    pub person: String,
    pub ops: Vec<String>,
    /// The term asked for. `None` is [`DEFAULT_HOURS`].
    pub hours: Option<u64>,
}

/// What the agent keeps from its request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requested {
    pub request: String,
    /// Shown once. The agent sends it back to redeem.
    pub binder: String,
    pub expires_at: u64,
}

/// What a redeem gives the route to mint.
pub struct Redeemed {
    pub spec: TokenSpec,
    pub delegation: String,
    pub space: String,
    pub ops: Vec<String>,
    pub expires: u64,
}

/// Why the agent's side did not succeed. Each is a distinct answer on the
/// wire **only** where the caller has proved something: a refusal a setting
/// explains, a request the binder unlocks.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// The deployment's settings refuse it. Names the setting and value.
    Setting(String),
    /// Too many requests are waiting: a cap a setting sets. Names the
    /// setting and the value that would admit it. Answered `429`.
    Limit(String),
    /// The body is not a request.
    Malformed(String),
    /// No request with this id and this binder. The one answer, whatever
    /// the cause, for a caller who proved nothing.
    Unknown,
    /// The person has not answered yet.
    Waiting { expires_at: u64 },
    /// The person said no.
    Declined,
    /// The request lapsed, or the delegation ended before it was redeemed.
    Expired,
    /// Already redeemed. A second redeem is a replay.
    Redeemed,
    /// The store could not be reached, with its typed class.
    Unavailable(ordo_identity::StoreFault),
}

// ── the seam ─────────────────────────────────────────────────────────────

/// One way of asking a person to let an agent act for them
/// (`plan-delegation-surface.md` §3.2). The HTTP door is the first
/// binding; MCP (11-4c) is the next, and needs only another caller of the
/// same methods.
///
/// What a binding does **not** decide: whether presence is required (the
/// door's step-up gate, ahead of `accept`), what the disclosure says, the
/// expiry ceiling, and what is recorded.
#[async_trait::async_trait]
pub trait DelegationSurface: Send + Sync {
    /// The agent asks. Creates no delegation.
    async fn request(&self, req: &AgentRequest) -> Result<Requested, Refusal>;
    /// What the person sees. Computed, never supplied by the agent.
    async fn disclosure(
        &self,
        space: &str,
        request: &str,
        person: &str,
    ) -> Result<Value, ExecError>;
    /// The person said yes. The one place a [`NewDelegation`] is built.
    async fn accept(&self, space: &str, request: &str, person: &str) -> Result<Value, ExecError>;
    /// The person said no.
    async fn decline(&self, space: &str, request: &str, person: &str) -> Result<Value, ExecError>;
    /// The agent redeems its request for a token. Fails until `accept`.
    async fn redeem(&self, request: &str, binder: &str) -> Result<Redeemed, Refusal>;
}

// ── the ceremony ─────────────────────────────────────────────────────────

/// The in-process implementation, over the deployment's own database.
pub struct Ceremony {
    pub pool: Pool,
    pub delegations: Arc<dyn DelegationRegistry>,
    pub grants: Arc<dyn GrantRegistry>,
    /// The engine the door decides with, read on every call.
    pub engine: Arc<dyn Fn() -> Arc<AccessEngine> + Send + Sync>,
    /// Where each tenant's program is read from, for the manifests.
    pub spaces: Arc<crate::spaces::Spaces>,
    pub stream: Option<Arc<Recorder>>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub settings: Settings,
}

fn not_found() -> ExecError {
    ExecError::new(
        ErrorCode::NotFound,
        "no open delegation request by that id is waiting for you",
    )
}

/// A store error as an op's: busy is `server_busy`, anything else
/// `space_unavailable` with the store's sentence.
fn unavailable(e: impl crate::auth::store_failure::IntoFault) -> ExecError {
    crate::auth::store_failure::exec_unavailable(e)
}

fn required<'a>(bound: &'a BoundParams, name: &str) -> Result<&'a str, ExecError> {
    bound
        .0
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ExecError::new(ErrorCode::MissingParam, format!("`${name}` is required")))
}

fn op_name_ok(op: &str) -> bool {
    !op.is_empty()
        && op.len() <= 128
        && op
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'$')
}

impl Ceremony {
    /// Create the request table. Called at boot beside the others.
    pub async fn ensure_table(pool: &Pool) -> Result<(), String> {
        for ddl in [REQUEST_DDL, REQUEST_INDEX_DDL, REQUEST_AGENT_INDEX_DDL] {
            pool.rows(ddl).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn at(&self) -> u64 {
        (self.now)()
    }

    /// Record one act. Returns the correlation id, which an approval stores
    /// as the delegation's `consent_correlation`.
    fn note(&self, space: Option<&str>, by: Option<&str>, act: &str, detail: String) -> String {
        let correlation = ordo_secevent::correlation::mint();
        let id = correlation.0.clone();
        if let Some(stream) = &self.stream {
            let (subject, rel) = match by {
                Some(who) => (Subject::member(who), Relationship::Member),
                None => (Subject::Unverified, Relationship::None),
            };
            stream.observe(
                Observation {
                    space: space.map(|s| ordo_commandlog::SpaceId(s.to_string())),
                    subject,
                    op: format!("delegation.{act}"),
                    kind: EventKind::Enrolment {
                        act: format!("delegation-{act}"),
                        detail,
                    },
                    at: ordo_commandlog::Timestamp(self.at()),
                    correlation,
                },
                rel,
            );
        }
        id
    }

    async fn rows(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, ordo_identity::StoreFault> {
        self.pool
            .rows(sql)
            .await
            .map_err(crate::auth::store_failure::IntoFault::fault)
    }

    async fn load(&self, space: &str, request: &str) -> Result<Option<RequestRow>, ExecError> {
        let sql = format!(
            r#"SELECT {COLUMNS} FROM "$delegation_request" WHERE "request_id" = {} AND "$space" = {}"#,
            lit(request),
            lit(space)
        );
        Ok(self
            .rows(&sql)
            .await
            .map_err(unavailable)?
            .first()
            .and_then(|r| row_of(r)))
    }

    /// The request, if it is open and names this person. Anything else is
    /// the one `not_found`, so a member cannot learn of a request meant for
    /// someone else.
    async fn open_for(
        &self,
        space: &str,
        request: &str,
        person: &str,
        op: &str,
    ) -> Result<RequestRow, ExecError> {
        let row = self.load(space, request).await?;
        let why = match &row {
            None => Some("unknown-request"),
            Some(r) if r.person != person => Some("not-the-person"),
            Some(r) if r.status != "waiting" => Some("not-waiting"),
            Some(r) if r.expires_at <= self.at() => Some("request-expired"),
            Some(_) => None,
        };
        match (row, why) {
            (Some(r), None) => Ok(r),
            (_, Some(reason)) => {
                self.note(
                    Some(space),
                    Some(person),
                    "refused",
                    format!("op={op} request={request} reason={reason} by={person}"),
                );
                Err(not_found())
            }
            (None, None) => Err(not_found()),
        }
    }

    async fn program(&self, space: &str) -> Result<Arc<Fixture>, ExecError> {
        self.spaces
            .resolve(space)
            .await
            .map(|b| b.view.fixture.clone())
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string()))
    }

    /// Whether `op` may be granted through this ceremony, and if not, why,
    /// in words for the person.
    pub fn grantable(engine: &AccessEngine, op: &str) -> Result<(), String> {
        if !engine.deployed().contains_key(op) {
            return Err("not an op of this app".into());
        }
        let platform = crate::platform_ops::specs();
        if platform.iter().any(|s| s.name == op) || op.starts_with("std.$") {
            return Err("a platform op, which is never delegated".into());
        }
        if !engine.delegable(op) {
            return Err(format!(
                "it deletes data, and the app developer has not opened it to agents. To open \
                 it, add it to {} ({}={op})",
                crate::delegable::ENV,
                crate::delegable::ENV
            ));
        }
        if engine.step_up_for(op, None).demands_presence() {
            return Err("it needs a fresh passkey touch, which only you can give".into());
        }
        Ok(())
    }

    /// The disclosure for one request, computed. Public so a test can
    /// assert it against the manifests directly.
    pub fn disclose(
        f: &Fixture,
        engine: &AccessEngine,
        settings: &Settings,
        row: &RequestRow,
        now: u64,
    ) -> Value {
        let space = SpaceId(row.space.clone());
        let manifests: BTreeMap<String, Value> = crate::manifests(f).into_iter().collect();
        let delegator = engine
            .principal_of_member(&space, &row.person)
            .cloned()
            .unwrap_or_else(|| PrincipalId(row.person.clone()));
        let mut ops = Vec::new();
        let mut not_granted = Vec::new();
        let mut unknown = 0usize;
        for op in &row.ops {
            let Some(m) = manifests.get(op) else {
                // Not an op of the program. The name is the agent's text,
                // so it is counted and never shown.
                unknown += 1;
                continue;
            };
            match Ceremony::grantable(engine, op) {
                Ok(()) => {
                    let mut d = describe_op(op, m);
                    let yours = engine.membership_reaches(&space, &delegator, op);
                    d["yours"] = json!(yours);
                    if !yours {
                        d["words"]
                            .as_array_mut()
                            .expect("words is an array")
                            .push(json!(
                                "You cannot call this yourself, so the agent cannot either."
                            ));
                    }
                    ops.push(d);
                }
                Err(why) => not_granted.push(json!({ "op": op, "reason": why })),
            }
        }
        let expires = now + row.term_secs;
        let owner = engine
            .spaces_of(&delegator)
            .into_iter()
            .any(|(s, role, _)| s == space && role == Some(Role::Owner));
        let agent_name = settings
            .agents
            .get(&row.agent)
            .cloned()
            .unwrap_or_else(|| row.agent.clone());
        json!({
            "request": row.id,
            "tenant": row.space,
            "agent": {
                "id": row.agent,
                "name": agent_name,
                "namedBy": "the app developer",
            },
            "person": row.person,
            "term": {
                "hours": row.term_secs / 3600,
                "expiresIfApprovedNow": expires,
                "expiresText": utc_text(expires),
            },
            "requestExpiresAt": row.expires_at,
            "ops": ops,
            "notGranted": not_granted,
            "unknownNames": unknown,
            "cannot": [
                "Call any op not listed above.",
                "Do anything that needs a fresh passkey touch.",
                "Delete data, unless the app developer opened that op to agents.",
                "Change members, roles or the tenant itself.",
                "Grant anyone else access.",
            ],
            "ceiling": "It can never do more than you can. If your role changes, so does what it can do.",
            "revoke": "You can end this at any time from your list of delegations (std.delegation.revoke). The agent's next call is refused.",
            "ownerSees": if owner {
                Value::Null
            } else {
                json!("The tenant's owner can see this delegation and what the agent does.")
            },
        })
    }
}

/// Plain words for one op, from its catalog manifest: what it reads, what it
/// changes, what the platform looks at on your behalf, and what kind of
/// change each statement makes.
pub fn describe_op(op: &str, m: &Value) -> Value {
    let list = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let mut examines = Vec::new();
    let mut words = Vec::new();
    for s in m["scope"].as_array().cloned().unwrap_or_default() {
        let entity = s["entity"].as_str().unwrap_or("?").to_string();
        let read = list(&s["read"]);
        let written = list(&s["written"]);
        if s["via"].as_str() == Some("guard") {
            if !read.is_empty() {
                words.push(format!(
                    "Looks at {} in {entity} through a row guard, to decide which rows it may touch.",
                    read.join(", ")
                ));
                examines.push(json!({ "entity": entity, "fields": read }));
            }
            continue;
        }
        if !read.is_empty() {
            words.push(format!("Reads {} in {entity}.", read.join(", ")));
            reads.push(json!({ "entity": entity, "fields": read }));
        }
        if !written.is_empty() {
            words.push(format!("Changes {} in {entity}.", written.join(", ")));
            writes.push(json!({ "entity": entity, "fields": written }));
        }
    }
    let mut deletes = Vec::new();
    for st in m["statements"].as_array().cloned().unwrap_or_default() {
        let entity = st["entity"].as_str().unwrap_or("?");
        if st["verb"].as_str() == Some("del") {
            deletes.push(entity.to_string());
            words.push(format!("Deletes rows in {entity}."));
        }
    }
    if m["unguarded"].as_bool().unwrap_or(false) {
        words.push("Sees every row, whoever asks: this op skips row guards.".to_string());
    }
    if let Some(purpose) = m["release"]["purpose"].as_str() {
        words.push(format!("Opens sealed fields ({purpose})."));
    }
    let kind = m["kind"].as_str().unwrap_or("read");
    if words.is_empty() {
        words.push(if kind == "read" {
            "Reads nothing stored.".to_string()
        } else {
            "Changes nothing stored.".to_string()
        });
    }
    json!({
        "op": op,
        "kind": kind,
        "reads": reads,
        "writes": writes,
        "examines": examines,
        "deletes": deletes,
        "words": words,
    })
}

/// `2026-10-10 14:00 UTC`, from seconds since the epoch.
pub fn utc_text(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60
    )
}

#[async_trait::async_trait]
impl DelegationSurface for Ceremony {
    async fn request(&self, req: &AgentRequest) -> Result<Requested, Refusal> {
        let now = self.at();
        if self.settings.agents.is_empty() {
            return Err(Refusal::Setting(format!(
                "this deployment accepts no delegation requests: {AGENTS_ENV} is unset. \
                 Name the agent there, as `{AGENTS_ENV}={}=<the name people see>`",
                req.agent
            )));
        }
        if !self.settings.agents.contains_key(&req.agent) {
            return Err(Refusal::Setting(format!(
                "`{}` is not an agent this deployment accepts requests for. {AGENTS_ENV} \
                 names: {}. To accept it, add `{}=<the name people see>` to {AGENTS_ENV}",
                req.agent,
                self.settings
                    .agents
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
                req.agent
            )));
        }
        if !ordo_api::well_formed_space(&req.space) {
            return Err(Refusal::Malformed("`space` is not a tenant id".into()));
        }
        if !ordo_api::well_formed_space(&req.person) {
            return Err(Refusal::Malformed("`person` is not a member id".into()));
        }
        let ops: BTreeSet<String> = req.ops.iter().map(|o| o.trim().to_string()).collect();
        if ops.is_empty() {
            return Err(Refusal::Malformed("`ops` names no op".into()));
        }
        if let Some(bad) = ops.iter().find(|o| !op_name_ok(o)) {
            let shown: String = bad.chars().take(40).collect();
            return Err(Refusal::Malformed(format!("`{shown}` is not an op name")));
        }
        if ops.len() > self.settings.max_ops {
            let (n, cap, hi) = (ops.len(), self.settings.max_ops, MAX_OPS_RANGE.1);
            return Err(Refusal::Setting(if n as u64 > hi {
                format!(
                    "the request names {n} ops, over {MAX_OPS_ENV}={cap} and over the most \
                     the setting allows ({hi}); ask for fewer ops"
                )
            } else {
                format!(
                    "the request names {n} ops, over {MAX_OPS_ENV}={cap}; ask for {cap} or \
                     fewer, or set {MAX_OPS_ENV}={n}"
                )
            }));
        }
        let hours = req
            .hours
            .unwrap_or(DEFAULT_HOURS.min(self.settings.max_hours));
        if hours == 0 {
            return Err(Refusal::Malformed("`hours` is at least 1".into()));
        }
        if hours > self.settings.max_hours {
            let (cap, hi) = (self.settings.max_hours, MAX_HOURS_RANGE.1);
            return Err(Refusal::Setting(if hours > hi {
                format!(
                    "the request asks for {hours} hours, over {MAX_HOURS_ENV}={cap} and over \
                     the most the setting allows ({hi}); ask for {cap} or fewer"
                )
            } else {
                format!(
                    "the request asks for {hours} hours, over {MAX_HOURS_ENV}={cap}; ask for \
                     {cap} or fewer, or set {MAX_HOURS_ENV}={hours}"
                )
            }));
        }
        // Clear what lapsed a day ago, so an open route cannot grow the
        // table without bound. A redeemed or decided row is kept as long.
        let _ = self
            .rows(&format!(
                r#"DELETE FROM "$delegation_request" WHERE "expires_at" < {}"#,
                now.saturating_sub(86_400)
            ))
            .await;
        // The route takes no token, so what one caller can leave waiting is
        // capped: per agent, and per person (audit 2026-10-11, finding 28).
        // The person's count is of requests naming that id, whether or not
        // such a member exists, so the answer discloses nothing.
        let waiting = |filter: String| {
            format!(
                r#"SELECT count(*) FROM "$delegation_request"
                   WHERE "status" = 'waiting' AND "expires_at" > {now} AND {filter}"#
            )
        };
        let count = |rows: Vec<Vec<Option<String>>>| -> u64 {
            rows.first()
                .and_then(|r| r.first().cloned().flatten())
                .and_then(|n| n.parse().ok())
                .unwrap_or(0)
        };
        let by_agent = count(
            self.rows(&waiting(format!(r#""agent" = {}"#, lit(&req.agent))))
                .await
                .map_err(Refusal::Unavailable)?,
        );
        if by_agent >= self.settings.open_per_agent {
            let cap = self.settings.open_per_agent;
            return Err(Refusal::Limit(format!(
                "agent `{}` has {by_agent} requests waiting, the most {OPEN_PER_AGENT_ENV}={cap} \
                 allows; wait for one to be decided or lapse, or set {OPEN_PER_AGENT_ENV}={}",
                req.agent,
                (cap + 1).max(by_agent + 1)
            )));
        }
        let by_person = count(
            self.rows(&waiting(format!(
                r#""$space" = {} AND "person" = {}"#,
                lit(&req.space),
                lit(&req.person)
            )))
            .await
            .map_err(Refusal::Unavailable)?,
        );
        if by_person >= self.settings.open_per_person {
            let cap = self.settings.open_per_person;
            return Err(Refusal::Limit(format!(
                "this person has {by_person} requests waiting, the most \
                 {OPEN_PER_PERSON_ENV}={cap} allows; wait for one to be decided or lapse, or \
                 set {OPEN_PER_PERSON_ENV}={}",
                (cap + 1).max(by_person + 1)
            )));
        }
        let id = format!("dr_{}", random_hex(16));
        let binder = random_hex(32);
        let expires_at = now + self.settings.request_secs;
        let list = ops.iter().cloned().collect::<Vec<_>>().join(",");
        let sql = format!(
            r#"INSERT INTO "$delegation_request"
               ("request_id", "$space", "agent", "person", "ops", "term_secs", "binder",
                "created_at", "expires_at", "status")
               VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, 'waiting')"#,
            lit(&id),
            lit(&req.space),
            lit(&req.agent),
            lit(&req.person),
            lit(&list),
            hours * 3600,
            lit(&sha256_hex(&binder)),
            now,
            expires_at,
        );
        self.rows(&sql).await.map_err(Refusal::Unavailable)?;
        // The agent proved nothing, so the act goes to the deployment's own
        // stream and does not grow the tenant's.
        self.note(
            None,
            None,
            "requested",
            format!(
                "request={id} tenant={} agent={} person={} ops={list} hours={hours}",
                req.space, req.agent, req.person
            ),
        );
        Ok(Requested {
            request: id,
            binder,
            expires_at,
        })
    }

    async fn disclosure(
        &self,
        space: &str,
        request: &str,
        person: &str,
    ) -> Result<Value, ExecError> {
        let row = self.open_for(space, request, person, REVIEW).await?;
        let f = self.program(space).await?;
        let engine = (self.engine)();
        Ok(Ceremony::disclose(
            &f,
            &engine,
            &self.settings,
            &row,
            self.at(),
        ))
    }

    async fn accept(&self, space: &str, request: &str, person: &str) -> Result<Value, ExecError> {
        let row = self.open_for(space, request, person, APPROVE).await?;
        let engine = (self.engine)();
        let ispace = SpaceId(space.to_string());
        let granted: BTreeSet<String> = row
            .ops
            .iter()
            .filter(|op| Ceremony::grantable(&engine, op).is_ok())
            .cloned()
            .collect();
        if granted.is_empty() {
            self.note(
                Some(space),
                Some(person),
                "refused",
                format!("op={APPROVE} request={request} reason=nothing-grantable by={person}"),
            );
            // Each op of the app with its reason. A name that is not an op
            // of the app is the agent's text, so it is counted, never shown.
            let reasons: Vec<String> = row
                .ops
                .iter()
                .filter(|op| engine.deployed().contains_key(op.as_str()))
                .filter_map(|op| {
                    Ceremony::grantable(&engine, op)
                        .err()
                        .map(|why| format!("`{op}`: {why}"))
                })
                .collect();
            let unknown = row.ops.len() - reasons.len();
            let mut message =
                "none of the ops this request names can be delegated here".to_string();
            if !reasons.is_empty() {
                message.push_str(". ");
                message.push_str(&reasons.join("; "));
            }
            if unknown > 0 {
                message.push_str(&format!(". {unknown} named op(s) are not ops of this app"));
            }
            return Err(ExecError::new(ErrorCode::TenantRefused, message));
        }
        let delegator = engine
            .principal_of_member(&ispace, person)
            .cloned()
            .unwrap_or_else(|| PrincipalId(person.to_string()));
        let agent = PrincipalId(row.agent.clone());
        let now = self.at();
        let expires = Timestamp(now + row.term_secs);
        let delegation = format!("dlg_{}", random_hex(12));
        let grant_id = format!("gr_{delegation}");
        let list = granted.iter().cloned().collect::<Vec<_>>().join(",");

        // Claim the request first, in one statement: of two approvals racing,
        // one moves the row and the other finds it gone. The agent's redeem
        // window starts again from here.
        let claim = format!(
            r#"UPDATE "$delegation_request"
               SET "status" = 'approved', "decided_at" = {now}, "delegator" = {},
                   "delegation_id" = {}, "grant_id" = {}, "granted_ops" = {},
                   "delegation_expires" = {}, "expires_at" = {}
               WHERE "request_id" = {} AND "$space" = {} AND "status" = 'waiting'
                 AND "expires_at" > {now}
               RETURNING "request_id""#,
            lit(&delegator.0),
            lit(&delegation),
            lit(&grant_id),
            lit(&list),
            expires.0,
            now + self.settings.request_secs,
            lit(request),
            lit(space),
        );
        if self.rows(&claim).await.map_err(unavailable)?.is_empty() {
            return Err(not_found());
        }
        let fail = |why: String| async move {
            let _ = self
                .rows(&format!(
                    r#"UPDATE "$delegation_request" SET "status" = 'failed' WHERE "request_id" = {}"#,
                    lit(request)
                ))
                .await;
            unavailable(why)
        };

        // Re-consent is revoke-then-record: one live delegation per person
        // and agent in a tenant keeps the audit join exact.
        let mut replaced = Vec::new();
        let live = match self.delegations.live_in(&ispace, Timestamp(now)).await {
            Ok(l) => l,
            Err(e) => return Err(fail(e.to_string()).await),
        };
        for d in live
            .iter()
            .filter(|d| d.delegator == delegator && d.agent == agent)
        {
            if let Err(e) = self
                .delegations
                .revoke(&ispace, &d.id, Timestamp(now))
                .await
            {
                return Err(fail(e.to_string()).await);
            }
            self.revoke_grant_of(space, &d.id.0, now).await;
            replaced.push(d.id.0.clone());
            self.note(
                Some(space),
                Some(person),
                "revoked",
                format!(
                    "delegation={} agent={} reason=replaced-by-{delegation} by={person}",
                    d.id.0, row.agent
                ),
            );
        }

        let correlation = self.note(
            Some(space),
            Some(person),
            "approved",
            format!(
                "request={request} delegation={delegation} agent={} ops={list} expires={} by={person}",
                row.agent, expires.0
            ),
        );
        let Some(grant) =
            engine.grant_for(GrantId(grant_id.clone()), &ispace, granted.clone(), expires)
        else {
            return Err(fail("an op changed while the request was approved".into()).await);
        };
        if let Err(e) = self
            .delegations
            .record(&NewDelegation {
                id: DelegationId(delegation.clone()),
                space: ispace.clone(),
                delegator: delegator.clone(),
                agent: agent.clone(),
                granted_at: Timestamp(now),
                expires,
                consent_correlation: correlation,
            })
            .await
        {
            return Err(fail(e.to_string()).await);
        }
        // Both records land, or neither stands (plan M3).
        if let Err(e) = self
            .grants
            .record(&NewGrant {
                id: grant.id().clone(),
                space: ispace.clone(),
                principal: agent,
                client: None,
                pins: grant.pins().clone(),
                granted_at: Timestamp(now),
                expires,
            })
            .await
        {
            let _ = self
                .delegations
                .revoke(&ispace, &DelegationId(delegation.clone()), Timestamp(now))
                .await;
            return Err(fail(e.to_string()).await);
        }
        Ok(json!({
            "request": request,
            "delegation": delegation,
            "agent": row.agent,
            "ops": granted.into_iter().collect::<Vec<_>>(),
            "expiresAt": expires.0,
            "replaced": replaced,
            "outcome": "approved",
        }))
    }

    async fn decline(&self, space: &str, request: &str, person: &str) -> Result<Value, ExecError> {
        let row = self.open_for(space, request, person, DECLINE).await?;
        let now = self.at();
        let sql = format!(
            r#"UPDATE "$delegation_request" SET "status" = 'declined', "decided_at" = {now}
               WHERE "request_id" = {} AND "$space" = {} AND "status" = 'waiting'
               RETURNING "request_id""#,
            lit(request),
            lit(space)
        );
        if self.rows(&sql).await.map_err(unavailable)?.is_empty() {
            return Err(not_found());
        }
        self.note(
            Some(space),
            Some(person),
            "declined",
            format!(
                "request={request} agent={} ops={} by={person}",
                row.agent,
                row.ops.join(",")
            ),
        );
        Ok(json!({ "request": request, "outcome": "declined" }))
    }

    async fn redeem(&self, request: &str, binder: &str) -> Result<Redeemed, Refusal> {
        let now = self.at();
        let hash = sha256_hex(binder);
        let take = format!(
            r#"UPDATE "$delegation_request" SET "status" = 'redeemed'
               WHERE "request_id" = {} AND "binder" = {} AND "status" = 'approved'
                 AND "expires_at" > {now}
               RETURNING {COLUMNS}"#,
            lit(request),
            lit(&hash),
        );
        let taken = self.rows(&take).await.map_err(Refusal::Unavailable)?;
        let Some(row) = taken.first().and_then(|r| row_of(r)) else {
            // Nothing redeemed. Say why only to the holder of the binder.
            let look = format!(
                r#"SELECT {COLUMNS} FROM "$delegation_request"
                   WHERE "request_id" = {} AND "binder" = {}"#,
                lit(request),
                lit(&hash)
            );
            let found = self
                .rows(&look)
                .await
                .map_err(Refusal::Unavailable)?
                .first()
                .and_then(|r| row_of(r));
            let Some(row) = found else {
                self.note(
                    None,
                    None,
                    "refused",
                    format!("op=redeem request={request} reason=unknown-or-wrong-binder"),
                );
                return Err(Refusal::Unknown);
            };
            let (answer, reason) = match row.status.as_str() {
                "waiting" if row.expires_at > now => (
                    Refusal::Waiting {
                        expires_at: row.expires_at,
                    },
                    None,
                ),
                "declined" => (Refusal::Declined, None),
                "redeemed" => (Refusal::Redeemed, Some("replayed")),
                _ => (Refusal::Expired, Some("expired")),
            };
            if let Some(reason) = reason {
                self.note(
                    Some(&row.space),
                    None,
                    "refused",
                    format!(
                        "op=redeem request={request} agent={} reason={reason}",
                        row.agent
                    ),
                );
            }
            return Err(answer);
        };
        let (Some(delegation), Some(grant_id), Some(expires)) = (
            row.delegation.clone(),
            row.grant.clone(),
            row.delegation_expires,
        ) else {
            return Err(Refusal::Expired);
        };
        let ispace = SpaceId(row.space.clone());
        // The person may have revoked between yes and redeem.
        let record: Option<DelegationRecord> = self
            .delegations
            .live_in(&ispace, Timestamp(now))
            .await
            .map_err(|e| Refusal::Unavailable(crate::auth::store_failure::IntoFault::fault(e)))?
            .into_iter()
            .find(|d| d.id.0 == delegation);
        let Some(record) = record else {
            self.note(
                Some(&row.space),
                None,
                "refused",
                format!(
                    "op=redeem request={request} agent={} reason=delegation-ended",
                    row.agent
                ),
            );
            return Err(Refusal::Expired);
        };
        let engine = (self.engine)();
        let ops: BTreeSet<String> = row.granted.iter().cloned().collect();
        let Some(grant): Option<CapabilityGrant> =
            engine.grant_for(GrantId(grant_id), &ispace, ops.clone(), Timestamp(expires))
        else {
            return Err(Refusal::Expired);
        };
        self.note(
            Some(&row.space),
            None,
            "redeemed",
            format!(
                "request={request} delegation={delegation} agent={}",
                row.agent
            ),
        );
        Ok(Redeemed {
            spec: TokenSpec {
                principal: PrincipalId(row.agent.clone()),
                expires: Timestamp(expires),
                client: None,
                user_presence_at: None,
                capability: Some(grant),
                delegation: Some(record.claim()),
                session: None,
            },
            delegation,
            space: row.space,
            ops: ops.into_iter().collect(),
            expires,
        })
    }
}

/// The grants a delegation's consent minted for its agent, revoked, and
/// their ids. The one place a delegation's paired grant is ended, whoever
/// ends the delegation.
async fn revoke_paired_grants(
    pool: &Pool,
    grants: &dyn GrantRegistry,
    space: &str,
    delegation: &str,
    now: u64,
) -> Result<Vec<String>, ordo_identity::StoreFault> {
    let sql = format!(
        r#"SELECT "grant_id" FROM "$delegation_request"
           WHERE "$space" = {} AND "delegation_id" = {}"#,
        lit(space),
        lit(delegation)
    );
    let rows = pool
        .rows(&sql)
        .await
        .map_err(crate::auth::store_failure::IntoFault::fault)?;
    let mut ended = Vec::new();
    for g in rows
        .into_iter()
        .filter_map(|r| r.first().cloned().flatten())
    {
        grants
            .revoke(
                &SpaceId(space.to_string()),
                &GrantId(g.clone()),
                Timestamp(now),
            )
            .await
            .map_err(|e| ordo_identity::StoreFault::from(e.to_string()))?;
        ended.push(g);
    }
    Ok(ended)
}

/// The consent records of a tenant, for ending a person's delegations on
/// their behalf: an owner succession ends the departed owner's
/// (`crate::identity_ops`, owner 2026-10-24).
#[derive(Clone)]
pub struct Consents {
    /// Where `$delegation_request` is, which pairs a delegation with the
    /// grant its consent minted.
    pub pool: Pool,
    pub delegations: Arc<dyn DelegationRegistry>,
    pub grants: Arc<dyn GrantRegistry>,
}

/// One delegation ended on a person's behalf, and the grants to its agent
/// that ended with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndedDelegation {
    pub id: String,
    pub agent: String,
    pub grants: Vec<String>,
}

impl Consents {
    /// End every live delegation `delegator` granted in `space`, and the
    /// grant each one's consent minted for its agent, as
    /// `std.delegation.revoke` ends one. Answers what ended and, separately,
    /// what could not be ended and why, so a caller records both.
    pub async fn end_all_of(
        &self,
        space: &str,
        delegator: &str,
        now: u64,
    ) -> (Vec<EndedDelegation>, Vec<String>) {
        let ispace = SpaceId(space.to_string());
        let mut ended = Vec::new();
        let mut failed = Vec::new();
        let held = match self
            .delegations
            .granted_by(&ispace, &PrincipalId(delegator.to_string()))
            .await
        {
            Ok(h) => h,
            Err(_) => {
                failed.push(format!("delegations-of-{delegator}:unavailable"));
                return (ended, failed);
            }
        };
        for d in held.into_iter().filter(|d| d.is_live(Timestamp(now))) {
            if self
                .delegations
                .revoke(&ispace, &d.id, Timestamp(now))
                .await
                .is_err()
            {
                failed.push(format!("{}:unavailable", d.id.0));
                continue;
            }
            let grants =
                match revoke_paired_grants(&self.pool, self.grants.as_ref(), space, &d.id.0, now)
                    .await
                {
                    Ok(g) => g,
                    Err(_) => {
                        failed.push(format!("grants-of-{}:unavailable", d.id.0));
                        Vec::new()
                    }
                };
            ended.push(EndedDelegation {
                id: d.id.0.clone(),
                agent: d.agent.0.clone(),
                grants,
            });
        }
        (ended, failed)
    }
}

impl Ceremony {
    async fn revoke_grant_of(&self, space: &str, delegation: &str, now: u64) {
        let _ =
            revoke_paired_grants(&self.pool, self.grants.as_ref(), space, delegation, now).await;
    }

    fn principal(&self, engine: &AccessEngine, space: &str, member: &str) -> PrincipalId {
        engine
            .principal_of_member(&SpaceId(space.to_string()), member)
            .cloned()
            .unwrap_or_else(|| PrincipalId(member.to_string()))
    }

    /// `std.delegation.mine`: the caller's requests, then any delegation
    /// they granted another way, newest first.
    async fn mine(&self, space: &str, caller: &str) -> Result<ExecResult, ExecError> {
        let engine = (self.engine)();
        let me = self.principal(&engine, space, caller);
        let now = self.at();
        let ispace = SpaceId(space.to_string());
        let records: BTreeMap<String, DelegationRecord> = self
            .delegations
            .granted_by(&ispace, &me)
            .await
            .map_err(unavailable)?
            .into_iter()
            .map(|d| (d.id.0.clone(), d))
            .collect();
        let sql = format!(
            r#"SELECT {COLUMNS} FROM "$delegation_request"
               WHERE "$space" = {} AND "person" = {}
               ORDER BY "created_at" DESC, "request_id" LIMIT 200"#,
            lit(space),
            lit(caller)
        );
        let requests: Vec<RequestRow> = self
            .rows(&sql)
            .await
            .map_err(unavailable)?
            .iter()
            .filter_map(|r| row_of(r))
            .collect();
        let name = |agent: &str| {
            self.settings
                .agents
                .get(agent)
                .cloned()
                .unwrap_or_else(|| agent.to_string())
        };
        let state_of = |d: &DelegationRecord| {
            if d.revoked_at.is_some() {
                "revoked"
            } else if d.is_live(Timestamp(now)) {
                "live"
            } else {
                "expired"
            }
        };
        let mut seen = BTreeSet::new();
        let mut rows = Vec::new();
        for r in &requests {
            let record = r.delegation.as_ref().and_then(|d| records.get(d));
            if let Some(d) = &r.delegation {
                seen.insert(d.clone());
            }
            let state = match (r.status.as_str(), record) {
                (_, Some(d)) => state_of(d),
                ("waiting", _) if r.expires_at > now => "waiting",
                ("declined", _) => "declined",
                ("failed", _) => "failed",
                _ => "expired",
            };
            rows.push(json!({
                "request": r.id,
                "delegation": r.delegation,
                "agent": r.agent,
                "agentName": name(&r.agent),
                "state": state,
                "ops": if r.granted.is_empty() { r.ops.clone() } else { r.granted.clone() },
                "grantedAt": record.map(|d| d.granted_at.0),
                "expiresAt": record.map(|d| d.expires.0).unwrap_or(r.expires_at),
                "revokedAt": record.and_then(|d| d.revoked_at.map(|t| t.0)),
            }));
        }
        for d in records.values().filter(|d| !seen.contains(&d.id.0)) {
            rows.push(json!({
                "request": Value::Null,
                "delegation": d.id.0,
                "agent": d.agent.0,
                "agentName": name(&d.agent.0),
                "state": state_of(d),
                "ops": Vec::<String>::new(),
                "grantedAt": d.granted_at.0,
                "expiresAt": d.expires.0,
                "revokedAt": d.revoked_at.map(|t| t.0),
            }));
        }
        Ok(ExecResult {
            position: None,
            rows,
            next_cursor: None,
        })
    }

    /// `std.delegation.revoke`: only the person who granted it.
    async fn revoke(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let id = required(bound, "delegation")?;
        let engine = (self.engine)();
        let me = self.principal(&engine, space, caller);
        let ispace = SpaceId(space.to_string());
        let now = self.at();
        let mine = self
            .delegations
            .granted_by(&ispace, &me)
            .await
            .map_err(unavailable)?
            .into_iter()
            .find(|d| d.id.0 == id);
        let Some(d) = mine else {
            self.note(
                Some(space),
                Some(caller),
                "refused",
                format!("op={REVOKE} delegation={id} reason=not-yours-or-unknown by={caller}"),
            );
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "you granted no delegation by that id in this tenant",
            ));
        };
        let outcome = if d.is_live(Timestamp(now)) {
            self.delegations
                .revoke(&ispace, &d.id, Timestamp(now))
                .await
                .map_err(unavailable)?;
            self.revoke_grant_of(space, id, now).await;
            self.note(
                Some(space),
                Some(caller),
                "revoked",
                format!("delegation={id} agent={} by={caller}", d.agent.0),
            );
            "revoked"
        } else {
            "already ended"
        };
        Ok(ExecResult {
            position: None,
            rows: vec![json!({ "delegation": id, "outcome": outcome })],
            next_cursor: None,
        })
    }

    /// Run one of the five ops. `caller` is the member the door proved.
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let one = |v: Value| ExecResult {
            position: None,
            rows: vec![v],
            next_cursor: None,
        };
        match op {
            REVIEW => {
                let request = required(bound, "request")?;
                self.disclosure(space, request, caller).await.map(one)
            }
            APPROVE => {
                let request = required(bound, "request")?;
                self.accept(space, request, caller).await.map(one)
            }
            DECLINE => {
                let request = required(bound, "request")?;
                self.decline(space, request, caller).await.map(one)
            }
            MINE => self.mine(space, caller).await,
            REVOKE => self.revoke(space, bound, caller).await,
            other => Err(ExecError::internal(format!(
                "`{other}` is not a delegation command"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_number_names_its_range_and_default() {
        for (name, bad, range, default) in [
            (MAX_HOURS_ENV, "0", "1 to 720", "8"),
            (REQUEST_SECS_ENV, "x", "60 to 3600", "600"),
            (MAX_OPS_ENV, "101", "1 to 100", "10"),
            (OPEN_PER_AGENT_ENV, "0", "1 to 1000000", "1000"),
            (OPEN_PER_PERSON_ENV, "1001", "1 to 1000", "10"),
        ] {
            let (var, detail) =
                Settings::from_vars(&|k| (k == name).then(|| bad.to_string())).unwrap_err();
            assert_eq!(var, name);
            assert!(detail.contains(range), "{detail}");
            assert!(detail.contains(&format!("default, {default}")), "{detail}");
        }
        let s = Settings::from_vars(&|_| None).unwrap();
        assert_eq!((s.open_per_agent, s.open_per_person), (1000, 10));
    }

    #[test]
    fn the_agent_list_reads_ids_and_names() {
        let a = Settings::parse_agents("agt_assistant=Expense assistant, agt_mail").unwrap();
        assert_eq!(a["agt_assistant"], "Expense assistant");
        assert_eq!(a["agt_mail"], "agt_mail");
        assert!(Settings::parse_agents("bad id=x").is_err());
        assert!(Settings::parse_agents("a=x,a=y").is_err());
    }

    #[test]
    fn the_defaults_accept_no_agent_and_cap_the_term() {
        let s = Settings::default();
        assert!(s.agents.is_empty());
        assert_eq!(s.max_hours, 8);
        assert_eq!(s.request_secs, 600);
        assert_eq!(s.max_ops, 10);
        assert!(s.describe().contains(AGENTS_ENV));
    }

    #[test]
    fn a_date_reads_as_a_date() {
        assert_eq!(utc_text(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_text(1_791_633_600), "2026-10-10 12:00 UTC");
    }

    #[test]
    fn approval_alone_asks_for_presence() {
        assert!(asks_presence(APPROVE));
        for op in [REVIEW, DECLINE, MINE, REVOKE] {
            assert!(!asks_presence(op));
            assert!(any_member(op));
        }
    }
}
