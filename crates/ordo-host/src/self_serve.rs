//! Built-in provisioning on sign-up: the setting, the two limits and the
//! hook (`plan-provisioning-on-signup.md` P2 and P3).
//!
//! # One setting, three layers
//!
//! `ORDO_PROVISION_SELF_SERVE` is `off` (the default) or `on-signup`. It is
//! stored in `$deploymentsetting` (the pattern guest sign-in shares): the
//! environment **seeds** it on a boot that finds nothing stored, the op
//! `std.deployment.setSelfServe` writes it, and after that the stored value
//! wins, so a change an app developer made through the op is not undone by
//! the next restart. The console will call the op.
//!
//! The limit follows the owner's principle (*"we do not enforce an app
//! developer's choices but we can have it as default"*): it is a
//! deployment default the app developer can change, never a fixed system
//! value.
//!
//! | Setting | Default | Environment | Op parameter |
//! |---|---|---|---|
//! | self-serve | `off` | `ORDO_PROVISION_SELF_SERVE` | `mode` |
//! | sign-ups an hour, whole deployment | 60 | `ORDO_PROVISION_SIGNUPS_PER_HOUR` | `signupsPerHour` |
//!
//! Self-serve provisioning does not depend on which sign-in method was used,
//! and does not read the platform-op presence list (owner, 2026-10-08).
//!
//! A limit is a whole number or the word `unlimited`. The default (60) is the
//! owner's, 2026-09-29.
//!
//! No limit here decides on a visitor's address, because behind a proxy the
//! socket shows the proxy's (owner, 2026-10-06). The per-address sign-up
//! limit, `ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR`, is proxy mode's
//! ([`crate::proxy`]): off by default, read only in proxy mode, and a
//! slow-down, never a refusal.
//!
//! # What the limits count
//!
//! Only while the setting is `on-signup`. Under `off` nothing here runs.
//!
//! - A **registration** without a ticket (`POST /auth/register`) is one
//!   sign-up. It is counted before the ceremony begins, so a refused sign-up
//!   mints nothing. A registration that presents a ticket is an invitation
//!   somebody in a tenant made, and is not counted.
//! - A **provision** (the hook giving a person a tenant, which costs a
//!   database) is counted separately, with the same limit.
//!
//! Both are a sliding hour, kept as rows in `$signup` (so every replica
//! counts the same rows) and checked under a lock in this process. Two
//! replicas checking at the same instant can each let one more through, so
//! the limit is exact per replica and over by at most the replica count.
//!
//! # The hook
//!
//! [`Signup::on_sign_in`] runs after `/auth/complete` has minted its token
//! and touches nothing before that. It runs on every sign-in under
//! `on-signup`, which is what makes a failed provision retryable and gives a
//! tenant to a person who registered before the setting was turned on.
//!
//! **It does not wait for the tenant** (owner, 2026-10-21: *"The process of
//! provisioning should not be a waiting call."*). It writes the tenant's
//! registry row and answers `provisioning: "in progress"`; the tenant is made
//! on a task of its own ([`TenantOps::start_own`]). The person's page reads
//! [`Signup::status`] through `GET /auth/provisioning` until it says
//! `ready`, and `POST /auth/provisioning` starts again after a failure.

use ordo_identity::StoreFault;
use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_exec::{sql_lit, Pool};
use serde_json::json;

use crate::tenant_ops::{OwnTenant, TenantOps};

pub const SET_SELF_SERVE: &str = "std.deployment.setSelfServe";

/// Every op of this module, for registration.
pub const OPS: [&str; 1] = [SET_SELF_SERVE];

pub const MODE_ENV: &str = "ORDO_PROVISION_SELF_SERVE";
pub const PER_HOUR_ENV: &str = "ORDO_PROVISION_SIGNUPS_PER_HOUR";

pub(crate) const MODE_KEY: &str = "provision_self_serve";
pub(crate) const PER_HOUR_KEY: &str = "provision_signups_per_hour";

/// The owner's defaults, 2026-09-29. A default, not a ceiling.
pub const DEFAULT_SIGNUPS_PER_HOUR: u64 = 60;

/// The window the limits count over.
const WINDOW_SECS: u64 = 3600;

/// `$signup`: one row per counted sign-up or provision.
pub const SIGNUP_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$signup" (
  "at"   bigint NOT NULL,
  "kind" text   NOT NULL
)"#;
const SIGNUP_INDEX: &str =
    r#"CREATE INDEX IF NOT EXISTS "$signup_kind_at" ON "$signup" ("kind", "at")"#;

/// Whether a person who signs in is given a tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelfServe {
    #[default]
    Off,
    OnSignup,
}

impl SelfServe {
    pub fn word(self) -> &'static str {
        match self {
            SelfServe::Off => "off",
            SelfServe::OnSignup => "on-signup",
        }
    }

    pub fn parse(s: &str) -> Result<SelfServe, String> {
        match s.trim() {
            "off" => Ok(SelfServe::Off),
            "on-signup" => Ok(SelfServe::OnSignup),
            other => Err(format!(
                "`{other}` is not `off` or `on-signup`, the two values self-serve provisioning \
                 takes"
            )),
        }
    }
}

/// A sign-up limit: a count an hour, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    PerHour(u64),
    Unlimited,
}

impl Limit {
    pub fn word(self) -> String {
        match self {
            Limit::PerHour(n) => n.to_string(),
            Limit::Unlimited => "unlimited".to_string(),
        }
    }

    pub fn parse(s: &str) -> Result<Limit, String> {
        let s = s.trim();
        if s == "unlimited" {
            return Ok(Limit::Unlimited);
        }
        match s.parse::<u64>() {
            Ok(n) if n > 0 => Ok(Limit::PerHour(n)),
            _ => Err(format!(
                "`{s}` is not a positive whole number or `unlimited`"
            )),
        }
    }

    fn allows(self, used: u64) -> bool {
        match self {
            Limit::PerHour(n) => used < n,
            Limit::Unlimited => true,
        }
    }
}

/// What the environment says at boot. `None` seeds nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelfServeConfig {
    pub mode: Option<SelfServe>,
    pub per_hour: Option<Limit>,
    /// The intervals and caps of setting up an own tenant (owner,
    /// 2026-10-21), read here so a bad value refuses the boot.
    pub setup: crate::tenant_setup::SetupSettings,
}

impl SelfServeConfig {
    /// Read the two variables. A value that does not parse refuses the
    /// boot and names its variable.
    pub fn from_env() -> Result<SelfServeConfig, (&'static str, String)> {
        let read = |var: &'static str| match std::env::var(var) {
            Ok(v) if !v.trim().is_empty() => Some(v),
            _ => None,
        };
        Ok(SelfServeConfig {
            mode: read(MODE_ENV)
                .map(|v| SelfServe::parse(&v))
                .transpose()
                .map_err(|e| (MODE_ENV, e))?,
            per_hour: read(PER_HOUR_ENV)
                .map(|v| Limit::parse(&v))
                .transpose()
                .map_err(|e| (PER_HOUR_ENV, e))?,
            setup: crate::tenant_setup::SetupSettings::from_env()?,
        })
    }
}

/// The settings in force: what is stored, else the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    pub mode: SelfServe,
    pub per_hour: Limit,
}

impl Default for Setting {
    fn default() -> Setting {
        Setting {
            mode: SelfServe::Off,
            per_hour: Limit::PerHour(DEFAULT_SIGNUPS_PER_HOUR),
        }
    }
}

pub async fn ensure_table(pool: &Pool) -> Result<(), ordo_exec::PoolError> {
    for ddl in [crate::admission::SETTING_DDL, SIGNUP_DDL, SIGNUP_INDEX] {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// The stored settings, over `$deploymentsetting`.
#[derive(Clone)]
pub struct SelfServeStore {
    pool: Pool,
}

impl SelfServeStore {
    pub fn new(pool: Pool) -> SelfServeStore {
        SelfServeStore { pool }
    }

    async fn stored(&self, key: &str) -> Result<Option<String>, StoreFault> {
        let rows = self
            .pool
            .rows(&format!(
                r#"SELECT "value" FROM "$deploymentsetting" WHERE "key" = {}"#,
                sql_lit(key)
            ))
            .await
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(
                    e,
                    "the provisioning setting could not be read",
                )
            })?;
        Ok(rows.first().and_then(|r| r[0].clone()))
    }

    async fn put(&self, key: &str, value: &str, by: &str, at: u64) -> Result<(), StoreFault> {
        self.pool
            .rows(&format!(
                r#"INSERT INTO "$deploymentsetting" ("key", "value", "updated_by", "updated_at")
                   VALUES ({}, {}, {}, {at})
                   ON CONFLICT ("key") DO UPDATE SET "value" = EXCLUDED."value",
                     "updated_by" = EXCLUDED."updated_by", "updated_at" = EXCLUDED."updated_at""#,
                sql_lit(key),
                sql_lit(value),
                sql_lit(by),
            ))
            .await
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(
                    e,
                    "the provisioning setting could not be written",
                )
            })?;
        Ok(())
    }

    /// The settings in force. A stored value that no longer parses reads as
    /// the default rather than failing every sign-in.
    pub async fn get(&self) -> Result<Setting, StoreFault> {
        let mut s = Setting::default();
        if let Some(v) = self.stored(MODE_KEY).await? {
            s.mode = SelfServe::parse(&v).unwrap_or_default();
        }
        if let Some(v) = self.stored(PER_HOUR_KEY).await? {
            s.per_hour = Limit::parse(&v).unwrap_or(s.per_hour);
        }
        Ok(s)
    }

    /// Change what is given; `None` leaves a setting alone.
    pub async fn set(
        &self,
        mode: Option<SelfServe>,
        per_hour: Option<Limit>,
        by: &str,
        at: u64,
    ) -> Result<Setting, StoreFault> {
        if let Some(m) = mode {
            self.put(MODE_KEY, m.word(), by, at).await?;
        }
        if let Some(l) = per_hour {
            self.put(PER_HOUR_KEY, &l.word(), by, at).await?;
        }
        self.get().await
    }

    /// What a boot does with the environment: each variable seeds its
    /// setting only when nothing is stored, and a variable that disagrees
    /// with what is stored is named and does not win. Returns the notes to
    /// print.
    pub async fn boot(&self, env: &SelfServeConfig, at: u64) -> Result<Vec<String>, StoreFault> {
        let mut notes = Vec::new();
        let items: [(&str, &str, Option<String>); 2] = [
            (MODE_KEY, MODE_ENV, env.mode.map(|m| m.word().to_string())),
            (PER_HOUR_KEY, PER_HOUR_ENV, env.per_hour.map(Limit::word)),
        ];
        for (key, var, wanted) in items {
            let Some(wanted) = wanted else { continue };
            match self.stored(key).await? {
                None => self.put(key, &wanted, "environment", at).await?,
                Some(now) if now != wanted => notes.push(format!(
                    "{var} disagrees with the stored setting, which is `{now}` and wins; change \
                     it with {SET_SELF_SERVE}"
                )),
                Some(_) => {}
            }
        }
        Ok(notes)
    }

    /// How many active own tenants the deployment holds, for the census and
    /// `/health/deep`.
    pub async fn own_tenants(&self) -> Option<u64> {
        count_own_tenants(&self.pool).await
    }
}

/// Active own tenants, `None` when the registry cannot be read.
pub async fn count_own_tenants(pool: &Pool) -> Option<u64> {
    let rows = pool
        .rows(
            r#"SELECT count(*) FROM "$spaces"
               WHERE "own_of" IS NOT NULL AND "state" = 'active'"#,
        )
        .await
        .ok()?;
    rows.first()?.first()?.as_deref()?.parse().ok()
}

/// The self-serve settings in force, for the deployment owner's status
/// surface (`/health/deep`, owner ruling 2026-10-05, `small-readbacks`).
/// The claim code it also named was removed (owner, 2026-10-30). `None`
/// when the settings cannot be read, so an unreadable setting is not shown
/// as `off`.
pub async fn readback(pool: &Pool) -> Option<serde_json::Value> {
    let s = SelfServeStore::new(pool.clone()).get().await.ok()?;
    Some(json!({
        "mode": s.mode.word(),
        "signupsPerHour": s.per_hour.word(),
    }))
}

/// The census line: the setting in force and how many own tenants exist.
pub async fn census_line(pool: &Pool) -> Option<String> {
    let s = SelfServeStore::new(pool.clone()).get().await.ok()?;
    let own = count_own_tenants(pool)
        .await
        .map_or_else(|| "unreadable".to_string(), |n| n.to_string());
    Some(match s.mode {
        SelfServe::Off => format!("self-serve off ({MODE_ENV}); {own} own tenants"),
        SelfServe::OnSignup => format!(
            "self-serve on-signup; {} sign-ups an hour; {own} own tenants",
            s.per_hour.word()
        ),
    })
}

/// What the ops of this module need: the store and the deployment's own
/// space, the only one the setting answers in.
pub struct SelfServeOps {
    pub store: SelfServeStore,
    pub home: String,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
}

impl SelfServeOps {
    /// `std.deployment.setSelfServe($mode?, $signupsPerHour?)`. With none of them it answers the settings and
    /// changes nothing.
    pub async fn execute(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if space != self.home {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "self-serve provisioning is the deployment's setting and this op answers only \
                 in the deployment's own space",
            ));
        }
        let text = |n: &str| {
            bound
                .0
                .get(n)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let refuse = |e: String| ExecError::new(ErrorCode::TenantRefused, e);
        let mode = text("mode")
            .map(SelfServe::parse)
            .transpose()
            .map_err(refuse)?;
        let per_hour = text("signupsPerHour")
            .map(Limit::parse)
            .transpose()
            .map_err(refuse)?;
        let now = (self.now)();
        let changed = mode.is_some() || per_hour.is_some();
        let s = self
            .store
            .set(mode, per_hour, caller, now)
            .await
            .map_err(ExecError::from)?;
        if changed {
            if let Some(stream) = &self.stream {
                crate::enrolment::record(
                    stream,
                    &crate::enrolment::Act {
                        act: "self-serve-set",
                        detail: format!(
                            "mode={} signups_per_hour={}",
                            s.mode.word(),
                            s.per_hour.word()
                        ),
                        by: Some(caller),
                        tenant: Some(space),
                        deployment: true,
                        at: now,
                    },
                );
            }
        }
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "mode": s.mode.word(),
                "signupsPerHour": s.per_hour.word(),
            })],
            next_cursor: None,
        })
    }
}

/// Which count a check is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counted {
    /// A registration ceremony without a ticket.
    Registration,
    /// The hook giving a person a tenant.
    Provision,
}

impl Counted {
    fn kind(self) -> &'static str {
        match self {
            Counted::Registration => "register",
            Counted::Provision => "provision",
        }
    }
}

/// A store error as the sign-up limit's refusal.
fn unavailable(e: impl crate::auth::store_failure::IntoFault) -> Refusal {
    Refusal::Unavailable(crate::auth::StoreFailure::from(e))
}

/// Why a sign-up was not counted in.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// The deployment-wide limit is reached.
    DeploymentFull,
    /// The settings or the count could not be read, and whether that was a
    /// full pool (`crate::auth::StoreFailure`).
    Unavailable(crate::auth::StoreFailure),
}

/// What the hook did, for the answer of `/auth/complete`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Given {
    /// The setting is `off`: nothing ran.
    Nothing,
    /// The person's own tenant, active.
    Space(String),
    /// Being built by another request.
    InProgress,
    /// The provision failed; the sign-in still succeeded.
    Failed,
    /// A sign-up limit is reached.
    LimitReached,
}

impl Given {
    /// The two fields of the answer.
    pub fn fields(&self) -> (Option<&str>, Option<&'static str>) {
        match self {
            Given::Nothing => (None, None),
            Given::Space(s) => (Some(s.as_str()), None),
            Given::InProgress => (None, Some("in progress")),
            Given::Failed => (None, Some("failed")),
            Given::LimitReached => (None, Some("limit reached")),
        }
    }
}

/// The hook's state, set once on the authentication state.
pub struct Signup {
    pub store: SelfServeStore,
    pub tenants: Arc<TenantOps>,
    /// Serialises check-then-count in this process.
    gate: tokio::sync::Mutex<()>,
}

impl Signup {
    pub fn new(store: SelfServeStore, tenants: Arc<TenantOps>) -> Signup {
        Signup {
            store,
            tenants,
            gate: tokio::sync::Mutex::new(()),
        }
    }

    /// The settings in force, or `None` when they cannot be read.
    pub async fn setting(&self) -> Option<Setting> {
        self.store.get().await.ok()
    }

    /// Count one sign-up or provision in, or refuse it. Answers `Ok` without
    /// counting when the setting is `off`.
    pub async fn admit(&self, what: Counted, now: u64) -> Result<(), Refusal> {
        let s = self.store.get().await.map_err(unavailable)?;
        if s.mode == SelfServe::Off {
            return Ok(());
        }
        self.count(what, now, &s).await
    }

    /// Count one new guest in, or refuse it, **whatever the mode**. The
    /// ceiling is the only bound on guest creation, since no limit reads a
    /// visitor's address (`plan-auth-federated.md` §7A), so it holds under
    /// `off` too. A guest is a sign-up, counted with the registrations.
    pub async fn admit_guest(&self, now: u64) -> Result<(), Refusal> {
        let s = self.store.get().await.map_err(unavailable)?;
        self.count(Counted::Registration, now, &s).await
    }

    async fn count(&self, what: Counted, now: u64, s: &Setting) -> Result<(), Refusal> {
        let pool = &self.store.pool;
        let kind = sql_lit(what.kind());
        let since = now.saturating_sub(WINDOW_SECS);
        let _one_at_a_time = self.gate.lock().await;
        pool.rows(&format!(r#"DELETE FROM "$signup" WHERE "at" <= {since}"#))
            .await
            .map_err(unavailable)?;
        let rows = pool
            .rows(&format!(
                r#"SELECT count(*) FROM "$signup" WHERE "kind" = {kind} AND "at" > {since}"#
            ))
            .await
            .map_err(unavailable)?;
        let total = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|c| c.parse::<u64>().ok())
            .unwrap_or(0);
        if !s.per_hour.allows(total) {
            return Err(Refusal::DeploymentFull);
        }
        pool.rows(&format!(
            r#"INSERT INTO "$signup" ("at", "kind") VALUES ({now}, {kind})"#
        ))
        .await
        .map_err(unavailable)?;
        Ok(())
    }

    /// The hook of `/auth/complete` (§2.3): give a person who signed in a
    /// tenant of their own, when the setting says so.
    ///
    /// Which sign-in method was used has no bearing (owner, 2026-10-08). The
    /// sign-in has succeeded whatever this answers: the identity is real
    /// whatever became of its tenant.
    pub async fn on_sign_in(&self, principal: &str, display: &str, now: u64) -> Given {
        let Ok(s) = self.store.get().await else {
            return Given::Failed;
        };
        if s.mode == SelfServe::Off {
            return Given::Nothing;
        }
        // A person who already has a tenant is answered with it and is not
        // counted: the limits protect the cost of making one.
        match crate::tenants::own_of(&self.store.pool, principal).await {
            Ok(Some(t)) => {
                return match t.state {
                    // A paused own tenant is still theirs (27ar).
                    crate::tenants::State::Active | crate::tenants::State::Paused => {
                        Given::Space(t.id)
                    }
                    _ => Given::InProgress,
                };
            }
            Ok(None) => {}
            Err(_) => return Given::Failed,
        }
        match self.count(Counted::Provision, now, &s).await {
            Ok(()) => {}
            Err(Refusal::Unavailable(_)) => return Given::Failed,
            Err(_) => return Given::LimitReached,
        }
        match self.tenants.start_own(principal, display).await {
            Ok(OwnTenant::Created { space, .. }) | Ok(OwnTenant::Existing { space }) => {
                Given::Space(space)
            }
            Ok(OwnTenant::InProgress { .. }) => Given::InProgress,
            Err(_) => Given::Failed,
        }
    }

    /// Where the person's own tenant stands. `serves` says whether the door
    /// on this replica serves the person in a space, which is what `ready`
    /// waits for: a tenant that is `active` and not yet in this replica's
    /// door would refuse the person's first call.
    pub async fn status(&self, principal: &str, serves: impl Fn(&str) -> bool) -> OwnStatus {
        let Ok(s) = self.store.get().await else {
            return OwnStatus::Unreadable;
        };
        let Ok(found) = crate::tenants::latest_own(&self.store.pool, principal).await else {
            return OwnStatus::Unreadable;
        };
        use crate::tenants::State;
        match found {
            None if s.mode == SelfServe::Off => OwnStatus::Off,
            None => OwnStatus::None,
            Some(t) => match t.state {
                State::Active | State::Paused if serves(&t.id) => OwnStatus::Ready { space: t.id },
                State::Active | State::Paused => OwnStatus::Provisioning {
                    space: t.id,
                    step: "serving",
                    stuck: false,
                    attempts: 0,
                },
                State::Provisioning => {
                    // Stuck by the same clock the sweep reads, so the person
                    // hears it as soon as the deployment owner does.
                    let now = crate::runtime::now_secs();
                    let after = self.tenants.setup().stuck_secs;
                    OwnStatus::Provisioning {
                        step: t.phase.as_str(),
                        stuck: t.stuck_at > 0 || now.saturating_sub(t.created_at) >= after,
                        attempts: t.attempts,
                        space: t.id,
                    }
                }
                State::Failed | State::Reaped => OwnStatus::Failed {
                    reason: failed_reason(t.phase),
                    space: t.id,
                },
            },
        }
    }

    /// `POST /auth/provisioning`: start the person's tenant again after a
    /// failure, or for the first time. A tenant already made or being made
    /// is left alone. Counted against the hour's provisions like a sign-in.
    pub async fn retry(&self, principal: &str, now: u64) -> Given {
        let name = match crate::tenants::latest_own(&self.store.pool, principal).await {
            Ok(Some(t)) => t.name,
            _ => String::new(),
        };
        self.on_sign_in(principal, &name, now).await
    }
}

/// Where a person's own tenant stands (`GET /auth/provisioning`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnStatus {
    /// Self-serve provisioning is `off` and the person has none.
    Off,
    /// The setting is on and the person has none yet: a sign-in, or a
    /// `POST /auth/provisioning`, starts one.
    None,
    /// Being made. `step` is the registry's phase, or `serving` for a tenant
    /// made and not yet in this replica's door. `stuck` once it has taken
    /// longer than `ORDO_PROVISION_STUCK_SECS`; it is still being tried.
    /// `attempts` is how many tries failed.
    Provisioning {
        space: String,
        step: &'static str,
        stuck: bool,
        attempts: u64,
    },
    /// Made, and this replica's door serves the person in it.
    Ready { space: String },
    /// It stopped. `reason` is said to the person; the registry's own note,
    /// which can name hosts, is only for the deployment owner.
    Failed { space: String, reason: String },
    /// The settings or the registry could not be read.
    Unreadable,
}

impl OwnStatus {
    /// The answer of `GET /auth/provisioning`.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            OwnStatus::Off => json!({"state": "off", "setting": MODE_ENV}),
            OwnStatus::None => json!({"state": "none"}),
            OwnStatus::Provisioning {
                space,
                step,
                stuck: false,
                attempts,
            } => {
                json!({"state": "provisioning", "space": space, "step": step, "stuck": false,
                       "attempts": attempts})
            }
            // Stuck is said to the person in words, and names the setting
            // that decides when, for the app developer reading the answer.
            OwnStatus::Provisioning {
                space,
                step,
                stuck: true,
                attempts,
            } => json!({
                "state": "provisioning",
                "space": space,
                "step": step,
                "stuck": true,
                "attempts": attempts,
                "reason": STUCK_REASON,
                "setting": crate::tenant_setup::STUCK_ENV,
            }),
            OwnStatus::Ready { space } => json!({"state": "ready", "space": space}),
            OwnStatus::Failed { space, reason } => {
                json!({"state": "failed", "space": space, "reason": reason})
            }
            OwnStatus::Unreadable => json!({"state": "unavailable"}),
        }
    }
}

/// What a person is told while setting up their tenant is stuck.
pub const STUCK_REASON: &str = "setting up your account is taking longer than it should. \
     It keeps trying on its own, and the deployment owner has been told. You can leave this \
     page and come back";

/// What a person is told about a provision that stopped, by how far it got.
fn failed_reason(phase: crate::tenants::Phase) -> String {
    use crate::tenants::Phase;
    // A phase names work done, so the step that stopped is the next one.
    let at = match phase {
        Phase::Reserved => "before anything was made",
        Phase::Targeted => "while its keys were being made",
        Phase::Custody => "while its database was being made",
        Phase::Database => "while its database was being connected",
        Phase::Attached => "while you were being added as its owner",
        Phase::Founded => "just before it opened",
    };
    format!(
        "setting up your account stopped {at}. Nothing you did caused it. Try again, and tell \
         the deployment owner if it keeps happening"
    )
}
