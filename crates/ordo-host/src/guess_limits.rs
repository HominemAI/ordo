//! Guessing limits: an always-on slowdown after wrong passwords, per account
//! and per share link, shared by every node (owner, 2026-10-25,
//! `guessing-a-password-or-a-link`).
//!
//! The ruling: "This should belong to the app dev, and even more so - a
//! tenant choice if multitenant environment is in use and these tenants have
//! their own sign in requirements, but we can set a default." The lock
//! stays off by default (2026-10-22). This only slows guessing.
//!
//! # What it does
//!
//! After `slowAfter` wrong tries on one account (or one link), each further
//! try waits for a doubling delay, 1 s, 2 s, 4 s, ..., never longer than
//! `slowCapSecs`. A try before the wait ends is refused `429
//! too_many_attempts` with `Retry-After`, and is neither checked nor counted.
//! The right password resets the count. It never locks: the rightful person
//! waits at most `slowCapSecs`.
//!
//! Every try is counted **before** the password is checked, in one guarded
//! statement, so a burst of tries sent at once cannot all slip under the
//! threshold. A try that then proves right takes its count back with the
//! reset.
//!
//! The count is keyed by the account (a hash of the identifier typed, known
//! or not, so the slowdown says nothing about which accounts exist) or by the
//! link, **never by an address** (owner, 2026-10-22: Ordo does not decide on
//! a client's address). It lives in `$guesscount` in the control database,
//! so two nodes share one count. A count untouched for a day starts again.
//!
//! # Layers
//!
//! The password slowdown is layered like a link's password limits
//! ([`crate::layered_limits`], [`crate::link_password`]):
//!
//! | Layer | Sets | How |
//! |---|---|---|
//! | Deployment | the defaults and absolute limits | `ORDO_PASSWORD_SLOW_AFTER`, `ORDO_PASSWORD_SLOW_CAP_SECS`, and a `_LIMIT` for each |
//! | App developer | its values, and optionally a range for tenants | `std.deployment.setPasswordSlowdown` |
//! | Tenant | its own values inside that range | `std.space.setPasswordSlowdown`, for sign-ins that name the tenant (`space`) |
//!
//! | Value | Looser is | `_LIMIT` is | Default, limit | Range |
//! |---|---|---|---|---|
//! | `slowAfter` | more | the most wrong tries before slowing | 5, 100 | `off` (0), then 1 to the limit |
//! | `slowCapSecs` | shorter | the shortest longest wait | 60, 1 | the limit to 1 day |
//!
//! The app developer may turn it off (`slowAfter=off`). A tenant may only
//! where the app developer's `slowAfterRange` starts at 0: a password account
//! is the deployment's, and a sign-in names its tenant, so a tenant's looser
//! value reaches every account signing in to it.
//!
//! A link's slowdown is the same two values among the link's password limits
//! (`std.space.setLinkPasswordLimits`), counted per link.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_exec::{sql_lit, Pool, PoolError};
use sha2::{Digest, Sha256};

use crate::layered_limits::{self as layered, Def, Family, Looser, OFF};

/// After this many wrong tries, a try waits.
pub const DEFAULT_SLOW_AFTER: u64 = 5;
/// The most wrong tries before slowing any layer may choose.
pub const DEFAULT_SLOW_AFTER_LIMIT: u64 = 100;
pub const SLOW_AFTER_CEILING: u64 = 1_000;
/// The longest wait.
pub const DEFAULT_SLOW_CAP_SECS: u64 = 60;
/// The shortest longest wait any layer may choose.
pub const DEFAULT_SLOW_CAP_SECS_LIMIT: u64 = 1;
pub const SLOW_CAP_SECS_CEILING: u64 = 86_400;
/// A count untouched this long starts again.
pub const FORGET_SECS: u64 = 86_400;

pub const SLOW_AFTER_ENV: &str = "ORDO_PASSWORD_SLOW_AFTER";
pub const SLOW_AFTER_LIMIT_ENV: &str = "ORDO_PASSWORD_SLOW_AFTER_LIMIT";
pub const SLOW_CAP_SECS_ENV: &str = "ORDO_PASSWORD_SLOW_CAP_SECS";
pub const SLOW_CAP_SECS_LIMIT_ENV: &str = "ORDO_PASSWORD_SLOW_CAP_SECS_LIMIT";

/// `std.deployment.setPasswordSlowdown`: the app developer's layer.
pub const SET_APP: &str = "std.deployment.setPasswordSlowdown";
/// `std.space.setPasswordSlowdown`: a tenant's own values.
pub const SET_TENANT: &str = "std.space.setPasswordSlowdown";
/// Every op of this module.
pub const OPS: [&str; 2] = [SET_APP, SET_TENANT];

/// Where the app developer's values are kept in `$deploymentsetting`.
pub(crate) const APP_KEY: &str = "password_slowdown";

/// What a count is of.
pub const KIND_PASSWORD: &str = "password";
pub const KIND_LINK: &str = "link";

/// How long the next try waits, after `misses` tries in a row that were not
/// the right one, counting this one. 0 is no wait.
pub fn delay(misses: u64, after: u64, cap: u64) -> u64 {
    if after == OFF || misses < after {
        return 0;
    }
    let n = (misses - after).min(40);
    (1u64 << n).min(cap.max(1))
}

// ── the password family ─────────────────────────────────────────────

/// One of the password slowdown's values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    SlowAfter,
    SlowCapSecs,
}

pub const FIELDS: [Field; 2] = [Field::SlowAfter, Field::SlowCapSecs];

static DEFS: [Def; 2] = [
    Def {
        word: "slowAfter",
        range_word: "slowAfterRange",
        from_word: "slowAfterFrom",
        range_from_word: "slowAfterRangeFrom",
        env: SLOW_AFTER_ENV,
        limit_env: SLOW_AFTER_LIMIT_ENV,
        column: "password_slow_after",
        default: DEFAULT_SLOW_AFTER,
        default_limit: DEFAULT_SLOW_AFTER_LIMIT,
        limit_span: (1, SLOW_AFTER_CEILING),
        looser: Looser::More,
        off: true,
        tenant_off: false,
    },
    Def {
        word: "slowCapSecs",
        range_word: "slowCapSecsRange",
        from_word: "slowCapSecsFrom",
        range_from_word: "slowCapSecsRangeFrom",
        env: SLOW_CAP_SECS_ENV,
        limit_env: SLOW_CAP_SECS_LIMIT_ENV,
        column: "password_slow_cap_secs",
        default: DEFAULT_SLOW_CAP_SECS,
        default_limit: DEFAULT_SLOW_CAP_SECS_LIMIT,
        limit_span: (1, SLOW_CAP_SECS_CEILING),
        looser: Looser::Less {
            ceiling: SLOW_CAP_SECS_CEILING,
        },
        off: false,
        tenant_off: false,
    },
];

impl Family for Field {
    const ALL: &'static [Field] = &FIELDS;
    const BAD: &'static str = "bad_password_slowdown";
    const OUT_OF_RANGE: &'static str = "password_slowdown_out_of_range";
    const APP_OP: &'static str = SET_APP;

    fn def(self) -> &'static Def {
        &DEFS[self.i()]
    }

    fn i(self) -> usize {
        match self {
            Field::SlowAfter => 0,
            Field::SlowCapSecs => 1,
        }
    }
}

pub type Deployment = layered::Deployment<Field>;
pub type Layer = layered::Layer<Field>;
pub type Effective = layered::Effective<Field>;

/// The tenant's columns on `$admissionpolicy`. `NULL` inherits.
pub const POLICY_DDL: &str = r#"ALTER TABLE "$admissionpolicy"
  ADD COLUMN IF NOT EXISTS "password_slow_after" bigint,
  ADD COLUMN IF NOT EXISTS "password_slow_cap_secs" bigint"#;

/// The counts. `key` is a hash of the identifier for a password, the link's
/// id for a link.
pub const COUNT_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$guesscount" (
  "kind"    text   NOT NULL,
  "key"     text   NOT NULL,
  "misses"  bigint NOT NULL DEFAULT 0,
  "next_at" bigint NOT NULL DEFAULT 0,
  "last_at" bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("kind","key")
)"#;

/// Create the counts table. Enough for a link's count.
pub async fn ensure_counts(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(COUNT_DDL).await.is_err() {
        pool.rows(COUNT_DDL).await?;
    }
    Ok(())
}

/// Create what the password slowdown keeps: the counts, the app
/// developer's row and the tenant's columns.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    crate::admission::ensure_table(pool).await?;
    for ddl in [crate::admission::SETTING_DDL, POLICY_DDL, COUNT_DDL] {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// The account a password try is counted against: a hash of what was
/// typed, so no address sits in the table.
pub fn account_key(identifier: &str) -> String {
    let mut h = Sha256::new();
    h.update(identifier.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

// ── the count ───────────────────────────────────────────────────────

/// What a try may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempt {
    /// Check the password. The try is already counted.
    Go,
    /// Refused unchecked: try again in this many seconds.
    Wait { retry_after: u64 },
}

/// The one client for the counts: Postgres in a deployment, a map in a
/// test. Every node over one database shares one count.
#[derive(Clone)]
pub struct Counter {
    backend: Arc<Backend>,
}

enum Backend {
    Pg { pool: Pool, sweeps: AtomicU64 },
    Memory(Mutex<BTreeMap<(String, String), Row>>),
}

#[derive(Debug, Clone, Copy)]
struct Row {
    misses: u64,
    next_at: u64,
    last_at: u64,
}

/// The most counts a process-local counter holds.
const MEMORY_MAX: usize = 100_000;

impl Counter {
    pub fn on_pool(pool: Pool) -> Counter {
        Counter {
            backend: Arc::new(Backend::Pg {
                pool,
                sweeps: AtomicU64::new(0),
            }),
        }
    }

    pub fn in_memory() -> Counter {
        Counter {
            backend: Arc::new(Backend::Memory(Mutex::new(BTreeMap::new()))),
        }
    }

    /// Count a try, or refuse it while its wait runs. With the slowdown off
    /// nothing is counted.
    pub async fn attempt(
        &self,
        kind: &str,
        key: &str,
        after: u64,
        cap: u64,
        now: u64,
    ) -> Result<Attempt, PoolError> {
        if after == OFF {
            return Ok(Attempt::Go);
        }
        let forget = now.saturating_sub(FORGET_SECS);
        match &*self.backend {
            Backend::Memory(m) => {
                let mut m = m.lock().expect("guess count mutex poisoned");
                if m.len() >= MEMORY_MAX {
                    m.retain(|_, r| r.last_at >= forget);
                    if m.len() >= MEMORY_MAX {
                        m.clear();
                    }
                }
                let r = m.entry((kind.to_string(), key.to_string())).or_insert(Row {
                    misses: 0,
                    next_at: 0,
                    last_at: now,
                });
                if r.next_at > now {
                    return Ok(Attempt::Wait {
                        retry_after: r.next_at - now,
                    });
                }
                let misses = if r.last_at < forget { 1 } else { r.misses + 1 };
                let wait = delay(misses, after, cap);
                *r = Row {
                    misses,
                    next_at: if wait > 0 { now + wait } else { 0 },
                    last_at: now,
                };
                Ok(Attempt::Go)
            }
            Backend::Pg { pool, sweeps } => {
                // Now and then, forget counts nobody has touched for a day.
                if sweeps.fetch_add(1, Ordering::Relaxed) % 256 == 255 {
                    let _ = pool
                        .rows(&format!(
                            r#"DELETE FROM "$guesscount" WHERE "last_at" < {forget}"#
                        ))
                        .await;
                }
                let (k, key) = (sql_lit(kind), sql_lit(key));
                let n = format!(r#"(CASE WHEN "last_at" < {forget} THEN 1 ELSE "misses" + 1 END)"#);
                for _ in 0..3 {
                    // A live row whose wait has ended: count this try, in
                    // one statement that holds the row.
                    let counted = pool
                        .rows(&format!(
                            r#"UPDATE "$guesscount" SET
                                 "misses" = {n},
                                 "next_at" = CASE WHEN {n} >= {after}
                                   THEN {now} + LEAST({cap}, power(2, LEAST({n} - {after}, 40))::bigint)
                                   ELSE 0 END,
                                 "last_at" = {now}
                               WHERE "kind" = {k} AND "key" = {key} AND "next_at" <= {now}
                               RETURNING "misses""#,
                            cap = cap.max(1),
                        ))
                        .await?;
                    if !counted.is_empty() {
                        return Ok(Attempt::Go);
                    }
                    // No row yet: the first try.
                    let first_wait = delay(1, after, cap);
                    let next_at = if first_wait > 0 { now + first_wait } else { 0 };
                    let made = pool
                        .rows(&format!(
                            r#"INSERT INTO "$guesscount" ("kind","key","misses","next_at","last_at")
                               VALUES ({k}, {key}, 1, {next_at}, {now})
                               ON CONFLICT ("kind","key") DO NOTHING RETURNING "misses""#
                        ))
                        .await?;
                    if !made.is_empty() {
                        return Ok(Attempt::Go);
                    }
                    // The row exists: either it is waiting, or another try
                    // moved it in between and this one goes round again.
                    let rows = pool
                        .rows(&format!(
                            r#"SELECT "next_at" FROM "$guesscount"
                               WHERE "kind" = {k} AND "key" = {key}"#
                        ))
                        .await?;
                    let next_at = rows
                        .first()
                        .and_then(|r| r.first())
                        .and_then(|c| c.as_deref())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0);
                    if next_at > now {
                        return Ok(Attempt::Wait {
                            retry_after: next_at - now,
                        });
                    }
                }
                // Three races in a row: refuse this one briefly rather than
                // let it through uncounted.
                Ok(Attempt::Wait { retry_after: 1 })
            }
        }
    }

    /// The right password: the count starts again.
    pub async fn reset(&self, kind: &str, key: &str) -> Result<(), PoolError> {
        match &*self.backend {
            Backend::Memory(m) => {
                m.lock()
                    .expect("guess count mutex poisoned")
                    .remove(&(kind.to_string(), key.to_string()));
                Ok(())
            }
            Backend::Pg { pool, .. } => {
                pool.rows(&format!(
                    r#"DELETE FROM "$guesscount" WHERE "kind" = {} AND "key" = {}"#,
                    sql_lit(kind),
                    sql_lit(key)
                ))
                .await?;
                Ok(())
            }
        }
    }
}

// ── the layers and their ops ────────────────────────────────────────

enum Store {
    Pg(Pool),
    Memory(Box<Mutex<(Layer, BTreeMap<String, Layer>)>>),
}

/// The password slowdown: its layers, the count, and the two setters.
pub struct GuessLimits {
    deployment: Deployment,
    store: Store,
    counter: Counter,
    home: Option<String>,
    stream: Option<Arc<ordo_secevent::Recorder>>,
}

fn unavailable(e: impl crate::auth::store_failure::IntoFault) -> ExecError {
    crate::auth::store_failure::exec_unavailable(e)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl GuessLimits {
    pub fn on_pool(deployment: Deployment, pool: Pool) -> GuessLimits {
        GuessLimits {
            deployment,
            counter: Counter::on_pool(pool.clone()),
            store: Store::Pg(pool),
            home: None,
            stream: None,
        }
    }

    /// One process's own: a test, or a deployment with no database.
    pub fn in_memory(deployment: Deployment) -> GuessLimits {
        GuessLimits {
            deployment,
            counter: Counter::in_memory(),
            store: Store::Memory(Box::new(Mutex::new((Layer::default(), BTreeMap::new())))),
            home: None,
            stream: None,
        }
    }

    pub fn with_home(mut self, home: impl Into<String>) -> GuessLimits {
        self.home = Some(home.into());
        self
    }

    pub fn with_stream(mut self, stream: Arc<ordo_secevent::Recorder>) -> GuessLimits {
        self.stream = Some(stream);
        self
    }

    pub fn deployment(&self) -> &Deployment {
        &self.deployment
    }

    pub fn counter(&self) -> &Counter {
        &self.counter
    }

    async fn app_layer(&self) -> Result<Layer, ordo_identity::StoreFault> {
        match &self.store {
            Store::Memory(m) => Ok(m.lock().expect("guess limits mutex poisoned").0),
            Store::Pg(pool) => {
                let rows = pool
                    .rows(&format!(
                        r#"SELECT "value" FROM "$deploymentsetting" WHERE "key" = {}"#,
                        sql_lit(APP_KEY)
                    ))
                    .await
                    .map_err(|e| {
                        crate::auth::store_failure::pool_fault(
                            e,
                            "the app developer's password slowdown could not be read",
                        )
                    })?;
                Ok(rows
                    .first()
                    .and_then(|r| r.first().cloned().flatten())
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                    .map(|v| Layer::from_json(&v))
                    .unwrap_or_default())
            }
        }
    }

    async fn tenant_layer(&self, space: &str) -> Result<Layer, ordo_identity::StoreFault> {
        match &self.store {
            Store::Memory(m) => Ok(m
                .lock()
                .expect("guess limits mutex poisoned")
                .1
                .get(space)
                .copied()
                .unwrap_or_default()),
            Store::Pg(pool) => {
                let cols: Vec<String> = FIELDS
                    .iter()
                    .map(|f| format!("\"{}\"", f.def().column))
                    .collect();
                let rows = pool
                    .rows(&format!(
                        r#"SELECT {} FROM "$admissionpolicy" WHERE "space" = {}"#,
                        cols.join(","),
                        sql_lit(space)
                    ))
                    .await
                    .map_err(|e| {
                        crate::auth::store_failure::pool_fault(
                            e,
                            "the tenant's password slowdown could not be read",
                        )
                    })?;
                Ok(Layer::from_columns(rows.first()))
            }
        }
    }

    /// The values in force for a sign-in naming `space`, or none. A store
    /// that cannot answer gives the deployment's: the slowdown stays on.
    pub async fn effective(&self, space: Option<&str>) -> Effective {
        let app = self.app_layer().await.unwrap_or_default();
        let tenant = match space {
            Some(s) => self.tenant_layer(s).await.unwrap_or_default(),
            None => Layer::default(),
        };
        layered::resolve(&self.deployment, &app, &tenant)
    }

    /// Count a password try against `identifier`, or refuse it while its
    /// wait runs. Answers the values in force too, for the refusal. A count
    /// that cannot be written lets the try through: the store being down is
    /// not the person's doing, and the ceremony's own store refuses next.
    pub async fn password_attempt(
        &self,
        identifier: &str,
        space: Option<&str>,
        now: u64,
    ) -> (Attempt, Effective) {
        let eff = self.effective(space).await;
        let attempt = self
            .counter
            .attempt(
                KIND_PASSWORD,
                &account_key(identifier),
                eff.get(Field::SlowAfter),
                eff.get(Field::SlowCapSecs),
                now,
            )
            .await
            .unwrap_or(Attempt::Go);
        (attempt, eff)
    }

    /// The right password for `identifier`.
    pub async fn password_right(&self, identifier: &str) {
        let _ = self
            .counter
            .reset(KIND_PASSWORD, &account_key(identifier))
            .await;
    }

    /// Run one of this module's ops.
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        match op {
            SET_APP => self.set_app(space, bound, caller).await,
            SET_TENANT => self.set_tenant(space, bound, caller).await,
            other => Err(ExecError::internal(format!(
                "`{other}` is not a password slowdown op"
            ))),
        }
    }

    fn answer(e: &Effective) -> ExecResult {
        ExecResult {
            position: None,
            rows: vec![e.row()],
            next_cursor: None,
        }
    }

    fn record(&self, space: Option<&str>, caller: &str, detail: String, now: u64) {
        if let Some(stream) = &self.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act: "password-slowdown-set",
                    detail,
                    by: Some(caller),
                    tenant: space,
                    deployment: space.is_none(),
                    at: now,
                },
            );
        }
    }

    async fn set_tenant(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let app = self.app_layer().await.map_err(unavailable)?;
        let mut t = self.tenant_layer(space).await.map_err(unavailable)?;
        for f in FIELDS {
            if let Some(change) = layered::value_change(bound, f)? {
                t.set_value(f, change);
            }
        }
        layered::check_tenant(&self.deployment, &app, &t)?;
        let now = now_secs();
        let opt = |v: Option<u64>| v.map_or("NULL".to_string(), |n| n.to_string());
        match &self.store {
            Store::Memory(m) => {
                m.lock()
                    .expect("guess limits mutex poisoned")
                    .1
                    .insert(space.to_string(), t);
            }
            Store::Pg(pool) => {
                let cols: Vec<String> = FIELDS
                    .iter()
                    .map(|f| format!("\"{}\"", f.def().column))
                    .collect();
                let vals: Vec<String> = FIELDS.iter().map(|f| opt(t.value(*f))).collect();
                let sets: Vec<String> = FIELDS
                    .iter()
                    .map(|f| format!("\"{0}\" = EXCLUDED.\"{0}\"", f.def().column))
                    .collect();
                pool.rows(&format!(
                    r#"INSERT INTO "$admissionpolicy" ("space",{},"updated_by","updated_at")
                       VALUES ({}, {}, {}, {now})
                       ON CONFLICT ("space") DO UPDATE SET {},
                         "updated_by" = EXCLUDED."updated_by",
                         "updated_at" = EXCLUDED."updated_at""#,
                    cols.join(","),
                    sql_lit(space),
                    vals.join(", "),
                    sql_lit(caller),
                    sets.join(", "),
                ))
                .await
                .map_err(unavailable)?;
            }
        }
        let detail: Vec<String> = FIELDS
            .iter()
            .map(|f| format!("{}={}", f.def().word, opt(t.value(*f))))
            .collect();
        self.record(
            Some(space),
            caller,
            format!("{} by={caller}", detail.join(" ")),
            now,
        );
        Ok(Self::answer(&layered::resolve(&self.deployment, &app, &t)))
    }

    async fn set_app(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if self.home.as_deref().is_some_and(|h| h != space) {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "the app developer's password slowdown is the deployment's setting, and this op \
                 answers only in the deployment's own space",
            ));
        }
        let mut a = self.app_layer().await.map_err(unavailable)?;
        for f in FIELDS {
            if let Some(change) = layered::value_change(bound, f)? {
                a.set_value(f, change);
            }
            if let Some(change) = layered::range_change(bound, f)? {
                a.set_range(f, change);
            }
        }
        layered::check_app(&self.deployment, &a)?;
        let now = now_secs();
        let value = a.to_json();
        match &self.store {
            Store::Memory(m) => m.lock().expect("guess limits mutex poisoned").0 = a,
            Store::Pg(pool) => {
                pool.rows(&format!(
                    r#"INSERT INTO "$deploymentsetting" ("key","value","updated_by","updated_at")
                       VALUES ({}, {}, {}, {now})
                       ON CONFLICT ("key") DO UPDATE SET "value" = EXCLUDED."value",
                         "updated_by" = EXCLUDED."updated_by",
                         "updated_at" = EXCLUDED."updated_at""#,
                    sql_lit(APP_KEY),
                    sql_lit(&value.to_string()),
                    sql_lit(caller),
                ))
                .await
                .map_err(unavailable)?;
            }
        }
        self.record(
            None,
            caller,
            format!("layer=app value={value} by={caller}"),
            now,
        );
        Ok(Self::answer(&layered::resolve(
            &self.deployment,
            &a,
            &Layer::default(),
        )))
    }

    /// The setter a refusal names: the tenant's when the tenant chose,
    /// else the app developer's.
    pub fn setting_of(e: &Effective) -> &'static str {
        match e.from_of(Field::SlowAfter) {
            "tenant" => SET_TENANT,
            _ => SET_APP,
        }
    }
}

/// The two setters' specs. Every parameter is optional text: a number,
/// `off` for `slowAfter`, `default` to clear, and for the app developer a
/// `lo-hi` range or `none`.
pub fn specs() -> Vec<ordo_api::OpSpec> {
    use ordo_api::{
        FieldSpec, NotSyncable, OpCost, OpKind, OpSpec, ParamSpec, ParamTy, RefineTier, Syncability,
    };
    let text = |name: &str, max: u32| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required: false,
        bounds: None,
    };
    let out = layered::answer_fields::<Field>();
    let spec = |name: &str, params: Vec<ParamSpec>| OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind: OpKind::Write,
        params,
        output_fields: out.iter().map(|f| FieldSpec::new(*f)).collect(),
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
        spec(
            SET_APP,
            FIELDS
                .iter()
                .flat_map(|f| [text(f.def().word, 16), text(f.def().range_word, 32)])
                .collect(),
        ),
        spec(
            SET_TENANT,
            FIELDS.iter().map(|f| text(f.def().word, 16)).collect(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_delay_doubles_after_the_threshold_up_to_the_cap() {
        let d: Vec<u64> = (1..=14).map(|m| delay(m, 5, 60)).collect();
        assert_eq!(d, [0, 0, 0, 0, 1, 2, 4, 8, 16, 32, 60, 60, 60, 60]);
        assert_eq!(delay(1_000, 5, 60), 60);
        assert_eq!(delay(9, OFF, 60), 0);
    }

    #[tokio::test]
    async fn a_count_waits_then_lets_the_right_answer_reset_it() {
        let c = Counter::in_memory();
        for _ in 0..5 {
            assert_eq!(
                c.attempt("password", "a", 5, 60, 100).await.unwrap(),
                Attempt::Go
            );
        }
        assert_eq!(
            c.attempt("password", "a", 5, 60, 100).await.unwrap(),
            Attempt::Wait { retry_after: 1 }
        );
        // Another account is not slowed.
        assert_eq!(
            c.attempt("password", "b", 5, 60, 100).await.unwrap(),
            Attempt::Go
        );
        assert_eq!(
            c.attempt("password", "a", 5, 60, 101).await.unwrap(),
            Attempt::Go
        );
        assert_eq!(
            c.attempt("password", "a", 5, 60, 101).await.unwrap(),
            Attempt::Wait { retry_after: 2 }
        );
        c.reset("password", "a").await.unwrap();
        assert_eq!(
            c.attempt("password", "a", 5, 60, 101).await.unwrap(),
            Attempt::Go
        );
    }

    #[test]
    fn a_tenant_cannot_turn_the_password_slowdown_off_unless_the_app_range_allows() {
        let d = Deployment::default();
        let mut t = Layer::default();
        t.set_value(Field::SlowAfter, Some(OFF));
        let e = layered::check_tenant(&d, &Layer::default(), &t).unwrap_err();
        assert!(e.to_string().contains(SET_APP), "{e}");
        let mut app = Layer::default();
        app.set_range(Field::SlowAfter, Some((0, 10)));
        assert!(layered::check_tenant(&d, &app, &t).is_ok());
    }
}
