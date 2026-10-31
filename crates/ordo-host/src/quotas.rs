//! `PgQuotas`: the door's [`ordo_api::QuotaGate`] over 17n M1's ledger.
//!
//! `plan-production-ops.md` M2. A quota has a limit and a grace: over the
//! limit alone warns and the request still serves; over limit **plus**
//! grace refuses with [`ordo_api::ErrorCode::QuotaExceeded`] (429). The gate
//! runs in `ordo_api::router::gate`, immediately after `AccessChecker::check`
//! and never before — a quota refusal handed to a caller with no membership
//! in the space would disclose that the space exists and is busy, which is
//! exactly what `access_denied`'s uniform answer exists to prevent
//! (`corpus-rulings.md`, `write-quota`, 2026-09-19).
//!
//! # Synchronous and pure, like `AccessChecker::check`
//!
//! [`PgQuotas::check`] never touches the database. It reads two in-memory
//! snapshots — the configured limits and the durable usage sums — that
//! [`PgQuotas::refresh`] rebuilds from `$quotas` and `$usage_replica` on a
//! tick, plus the live, not-yet-flushed counters `ordo_host::metrics::Metrics`
//! is already holding for [`crate::workers::UsageFlush`]. A database outage
//! therefore cannot become a door outage; the quota answer is only as fresh
//! as the last `refresh`, the same staleness `PgSessionRevocations` names for
//! the deny list.
//!
//! # v1 gates `requests` and `rows_returned` only
//!
//! Both are counted at the door as they happen (17n M1a), so the quota is
//! exact. `writes` is counted from the command log at rollup (M1c) and a
//! quota on it would bite up to a rollup period late, which is a quota in
//! name only — deferred to 18i, the post-deployment roadmap. `set_limit`
//! refuses any quantity outside [`QUANTITIES`] for the same reason a
//! malformed cadence refuses at boot: a typo'd quantity that silently never
//! matched anything would look configured and gate nothing.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use ordo_api::{OpSpec, QuotaDecision, QuotaGate};
use ordo_exec::{sql_lit, usage as usage_tables, Pool, PoolError};

use crate::metrics::Metrics;

/// Requests served through the door, by cost class summed together. Counted
/// exactly at the door (17n M1a); see the module doc.
pub const QUANTITY_REQUESTS: &str = "requests";
/// Rows a `Ran` outcome returned. Counted exactly at the door (17n M1a).
pub const QUANTITY_ROWS_RETURNED: &str = "rows_returned";
/// The whole v1 quantity set — `writes` is deliberately absent; see the
/// module doc and 18i.
pub const QUANTITIES: &[&str] = &[QUANTITY_REQUESTS, QUANTITY_ROWS_RETURNED];

/// `$quotas`: one row per `(space, quantity)`, the operator's own
/// configuration rather than anything the ledger writes. Control-database,
/// beside `$usage_replica` and `$usage` — it answers "what is this
/// deployment willing to serve", which is a fact about the deployment like
/// the usage tables are.
pub const QUOTA_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$quotas" (
  "space"       text   NOT NULL,
  "quantity"    text   NOT NULL,
  "limit"       bigint NOT NULL,
  "grace"       bigint NOT NULL,
  "period_secs" bigint NOT NULL,
  PRIMARY KEY ("space", "quantity")
)"#;

/// Create `$quotas`, at boot beside the other platform tables — always,
/// regardless of whether the ledger is on, so `ordo quota set` works against
/// a database an operator is configuring ahead of turning `ORDO_USAGE_LEDGER`
/// on.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(QUOTA_DDL).await.is_err() {
        pool.rows(QUOTA_DDL).await?;
    }
    Ok(())
}

/// One `(space, quantity)` row's configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub limit: u64,
    pub grace: u64,
    pub period_secs: u64,
}

/// Refuse a quantity `set_limit`/`ordo quota set` was not given one of
/// [`QUANTITIES`] for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownQuantity(pub String);

impl std::fmt::Display for UnknownQuantity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` is not a quota quantity — v1 gates {} (writes are 18i, on the \
             post-deployment roadmap)",
            self.0,
            QUANTITIES.join(" and "),
        )
    }
}

/// Why [`set_limit`] stored nothing. `ordo quota set` and
/// `std.deployment.setQuota` (G9) answer with the same words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaRefusal {
    /// A quantity outside [`QUANTITIES`].
    Quantity(UnknownQuantity),
    /// A period of 0 seconds: a quota that starts again every instant
    /// counts nothing.
    ZeroPeriod,
    /// The control database did not take the row.
    Database(String),
}

impl std::fmt::Display for QuotaRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuotaRefusal::Quantity(q) => q.fmt(f),
            QuotaRefusal::ZeroPeriod => write!(
                f,
                "a quota's period is at least 1 second: 3600 is an hour, 86400 a day"
            ),
            QuotaRefusal::Database(e) => write!(f, "database: {e}"),
        }
    }
}

fn quantity_static(q: &str) -> &'static str {
    match q {
        QUANTITY_REQUESTS => QUANTITY_REQUESTS,
        QUANTITY_ROWS_RETURNED => QUANTITY_ROWS_RETURNED,
        _ => "unknown",
    }
}

/// Set (or replace) one space's limit for one quantity. The one engine
/// behind `ordo quota set` and `std.deployment.setQuota` (G9): an upsert,
/// since setting a new number is the ordinary way to change a limit, not a
/// two-step remove-then-add.
pub async fn set_limit(
    pool: &Pool,
    space: &str,
    quantity: &str,
    limit: u64,
    grace: u64,
    period_secs: u64,
) -> Result<(), QuotaRefusal> {
    if !QUANTITIES.contains(&quantity) {
        return Err(QuotaRefusal::Quantity(UnknownQuantity(
            quantity.to_string(),
        )));
    }
    if period_secs == 0 {
        return Err(QuotaRefusal::ZeroPeriod);
    }
    // `bigint` is signed: a larger number is held as the largest it
    // takes, which is as good as no limit.
    let (limit, grace, period_secs) = (
        limit.min(i64::MAX as u64),
        grace.min(i64::MAX as u64),
        period_secs.min(i64::MAX as u64),
    );
    pool.rows(&format!(
        "INSERT INTO \"$quotas\" (\"space\", \"quantity\", \"limit\", \"grace\", \"period_secs\") \
         VALUES ({space}, {quantity}, {limit}, {grace}, {period_secs}) \
         ON CONFLICT (\"space\", \"quantity\") DO UPDATE SET \
           \"limit\" = EXCLUDED.\"limit\", \
           \"grace\" = EXCLUDED.\"grace\", \
           \"period_secs\" = EXCLUDED.\"period_secs\"",
        space = sql_lit(space),
        quantity = sql_lit(quantity),
        limit = limit,
        grace = grace,
        period_secs = period_secs,
    ))
    .await
    .map_err(|e| QuotaRefusal::Database(e.to_string()))?;
    Ok(())
}

/// Every quantity configured for one space, quantity-ordered — `ordo quota
/// show`'s read.
pub async fn limits_for(pool: &Pool, space: &str) -> Result<Vec<(String, Limit)>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"quantity\", \"limit\", \"grace\", \"period_secs\" FROM \"$quotas\" \
             WHERE \"space\" = {} ORDER BY \"quantity\"",
            sql_lit(space)
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some((
                r.first()?.clone()?,
                Limit {
                    limit: r.get(1)?.clone()?.parse().ok()?,
                    grace: r.get(2)?.clone()?.parse().ok()?,
                    period_secs: r.get(3)?.clone()?.parse().ok()?,
                },
            ))
        })
        .collect())
}

/// Every `(space, quantity)` this deployment has a limit configured for, for
/// [`PgQuotas::refresh`] to sum usage against.
async fn all_limits(pool: &Pool) -> Result<BTreeMap<(String, String), Limit>, PoolError> {
    let rows = pool
        .rows("SELECT \"space\", \"quantity\", \"limit\", \"grace\", \"period_secs\" FROM \"$quotas\"")
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let space = r.first()?.clone()?;
            let quantity = r.get(1)?.clone()?;
            let limit = Limit {
                limit: r.get(2)?.clone()?.parse().ok()?,
                grace: r.get(3)?.clone()?.parse().ok()?,
                period_secs: r.get(4)?.clone()?.parse().ok()?,
            };
            Some(((space, quantity), limit))
        })
        .collect())
}

/// `SUM` over `$usage_replica` for one space and quantity, within the
/// quantity's own current period — `period_of(now, period_secs)`, the same
/// alignment `usage.flush` and `usage.rollup` use, but on the quota's own
/// grain rather than the flush cadence's.
async fn durable_used(
    pool: &Pool,
    space: &str,
    quantity: &str,
    period_start: u64,
) -> Result<u64, PoolError> {
    let space_lit = sql_lit(space);
    let sql = match quantity {
        QUANTITY_REQUESTS => format!(
            "SELECT COALESCE(SUM((v.value)::bigint), 0) FROM \"$usage_replica\", \
             jsonb_each_text(\"requests\") AS v \
             WHERE \"space\" = {space_lit} AND \"period_start\" >= {period_start}"
        ),
        QUANTITY_ROWS_RETURNED => format!(
            "SELECT COALESCE(SUM(\"rows_returned\"), 0) FROM \"$usage_replica\" \
             WHERE \"space\" = {space_lit} AND \"period_start\" >= {period_start}"
        ),
        _ => return Ok(0),
    };
    let rows = pool.rows(&sql).await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// The Postgres-backed [`QuotaGate`]. `check` reads only [`PgQuotas::limits`]
/// and [`PgQuotas::used`] plus the live ledger — see the module doc.
pub struct PgQuotas {
    limits: RwLock<BTreeMap<(String, String), Limit>>,
    used: RwLock<BTreeMap<(String, String), u64>>,
    metrics: Arc<Metrics>,
}

impl PgQuotas {
    /// Starts with nothing configured — every space is `Within` until the
    /// first [`PgQuotas::refresh`] loads `$quotas`.
    pub fn new(metrics: Arc<Metrics>) -> PgQuotas {
        PgQuotas {
            limits: RwLock::new(BTreeMap::new()),
            used: RwLock::new(BTreeMap::new()),
            metrics,
        }
    }

    /// Reload the configured limits and their durable usage sums. Called by
    /// [`crate::workers::UsageFlush`] on every tick, **after** it writes this
    /// replica's rows — so a limit this replica's own traffic just crossed
    /// is visible on the very next request, not one flush interval later.
    pub async fn refresh(&self, pool: &Pool, now_secs: u64) -> Result<(), PoolError> {
        let limits = all_limits(pool).await?;
        let mut used = BTreeMap::new();
        for ((space, quantity), limit) in &limits {
            let (period_start, _) = usage_tables::period_of(now_secs, limit.period_secs.max(1));
            let sum = durable_used(pool, space, quantity, period_start).await?;
            used.insert((space.clone(), quantity.clone()), sum);
        }
        *unpoisoned(&self.limits) = limits;
        *unpoisoned(&self.used) = used;
        Ok(())
    }
}

fn unpoisoned<T>(m: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|e| e.into_inner())
}

fn unpoisoned_read<T>(m: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|e| e.into_inner())
}

/// The live half of a request's cost, for the one quantity `check` needs it
/// for: `requests` sums every cost class the door has already served this
/// replica since the last flush; `rows_returned` is that counter as-is.
fn live_count(metrics: &Metrics, space: &str, quantity: &str) -> u64 {
    let live = metrics.usage_peek(space);
    match quantity {
        QUANTITY_REQUESTS => live.requests.values().sum(),
        QUANTITY_ROWS_RETURNED => live.rows_returned,
        _ => 0,
    }
}

impl QuotaGate for PgQuotas {
    fn check(&self, space: &str, _op: &OpSpec) -> QuotaDecision {
        let limits = unpoisoned_read(&self.limits);
        let used = unpoisoned_read(&self.used);
        // `Exceeded` outranks `Warn` outranks `Within`: a space over one
        // quantity's grace is refused even if another of its quantities is
        // merely warning.
        let mut worst = QuotaDecision::Within;
        for ((s, quantity), limit) in limits.iter() {
            if s != space {
                continue;
            }
            let durable = used
                .get(&(s.clone(), quantity.clone()))
                .copied()
                .unwrap_or(0);
            let total = durable + live_count(&self.metrics, space, quantity);
            let name = quantity_static(quantity);
            let decision = if total >= limit.limit + limit.grace {
                QuotaDecision::Exceeded {
                    quantity: name,
                    used: total,
                    limit: limit.limit,
                    grace: limit.grace,
                }
            } else if total >= limit.limit {
                QuotaDecision::Warn {
                    quantity: name,
                    used: total,
                    limit: limit.limit,
                }
            } else {
                QuotaDecision::Within
            };
            if !matches!(decision, QuotaDecision::Within) {
                self.metrics.quota_over_limit(space, name);
            }
            worst = worse_of(worst, decision);
        }
        worst
    }
}

/// `Exceeded` beats `Warn` beats `Within` — the order [`PgQuotas::check`]
/// folds a space's several quantities down to the one decision the door
/// acts on.
fn worse_of(a: QuotaDecision, b: QuotaDecision) -> QuotaDecision {
    fn rank(d: &QuotaDecision) -> u8 {
        match d {
            QuotaDecision::Within => 0,
            QuotaDecision::Warn { .. } => 1,
            QuotaDecision::Exceeded { .. } => 2,
        }
    }
    if rank(&b) >= rank(&a) {
        b
    } else {
        a
    }
}

// ── G9: the quotas through the door ─────────────────────────────────────

/// `std.deployment.quotas($space?)`: every quota set, with what the space
/// has used of it this period.
pub const QUOTAS: &str = "std.deployment.quotas";
/// `std.deployment.setQuota($space, $quantity, $limit, $grace,
/// $periodSecs)`: `ordo quota set` through the door.
pub const SET_QUOTA: &str = "std.deployment.setQuota";
/// Every op this module serves. Both are the deployment owner's, in its own
/// space (`plan-console.md` 4.9 and 7, G9).
pub const OPS: [&str; 2] = [QUOTAS, SET_QUOTA];

/// The setting that turns quota enforcement on, named by every answer
/// given while it is off.
pub const LEDGER_ENV: &str = "ORDO_USAGE_LEDGER";

fn param(name: &str, ty: ordo_api::ParamTy, required: bool) -> ordo_api::ParamSpec {
    ordo_api::ParamSpec {
        name: name.to_string(),
        ty,
        required,
        bounds: None,
    }
}

fn spec(name: &str, kind: ordo_api::OpKind, params: Vec<ordo_api::ParamSpec>) -> OpSpec {
    OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind,
        params,
        output_fields: [
            "space",
            "quantity",
            "limit",
            "grace",
            "periodSecs",
            "periodStart",
            "used",
            "enforced",
        ]
        .iter()
        .map(|f| ordo_api::FieldSpec::new(*f))
        .collect(),
        live_capable: false,
        sync: ordo_api::Syncability::NotSyncable(ordo_api::NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: ordo_api::RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: ordo_api::OpCost::default(),
    }
}

/// The two ops' specs: a read with one optional filter, and a write.
pub fn specs() -> Vec<OpSpec> {
    use ordo_api::{OpKind, ParamTy};
    let text = || ParamTy::Text { max_len: Some(256) };
    vec![
        spec(QUOTAS, OpKind::Read, vec![param("space", text(), false)]),
        spec(
            SET_QUOTA,
            OpKind::Write,
            vec![
                param("space", text(), true),
                param("quantity", text(), true),
                param("limit", ParamTy::Int, true),
                param("grace", ParamTy::Int, true),
                param("periodSecs", ParamTy::Int, true),
            ],
        ),
    ]
}

/// What the two ops need.
pub struct QuotaOps {
    /// The control database: `$quotas`, `$usage_replica` and the registry.
    pub control: Pool,
    /// The deployment's own space, the only one the ops answer in.
    pub home: String,
    /// Whether this deployment enforces quotas: [`LEDGER_ENV`] is on. Off,
    /// a quota is stored and gates nothing until it is.
    pub enforced: bool,
    /// Where `setQuota` records the change (G12).
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

fn invalid(msg: impl Into<String>) -> ordo_api::ExecError {
    ordo_api::ExecError::new(ordo_api::ErrorCode::TenantRefused, msg)
}

fn unreadable(e: PoolError) -> ordo_api::ExecError {
    ordo_api::ExecError::of_fault(
        e.class(),
        ordo_api::ErrorCode::SpaceUnavailable,
        format!("the quotas could not be read or written ({e}); nothing was changed"),
    )
}

/// A whole number parameter, as a number or as the text of one.
fn whole(bound: &ordo_api::BoundParams, name: &str) -> Result<u64, ordo_api::ExecError> {
    let v = bound.0.get(name);
    v.and_then(|v| match v {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
    .ok_or_else(|| invalid(format!("`{name}` is a whole number, 0 or more")))
}

impl QuotaOps {
    /// Run `std.deployment.quotas` or `std.deployment.setQuota`.
    pub async fn execute(
        &self,
        name: &str,
        space: &str,
        bound: &ordo_api::BoundParams,
        caller: &str,
    ) -> Result<ordo_api::ExecResult, ordo_api::ExecError> {
        if space != self.home {
            // `not_found`, as every deployment op answers outside its home.
            return Err(ordo_api::ExecError::new(
                ordo_api::ErrorCode::NotFound,
                "quotas are the deployment's: these ops answer only in the deployment's own \
                 space",
            ));
        }
        let rows = match name {
            SET_QUOTA => vec![self.set(bound, caller).await?],
            _ => self.read(bound).await?,
        };
        Ok(ordo_api::ExecResult {
            position: None,
            rows,
            next_cursor: None,
        })
    }

    /// One row: the quota, its current period and what the flushed usage
    /// says was used in it. This node's requests since its last flush are
    /// not in `used`; the gate adds them.
    async fn row(
        &self,
        space: &str,
        quantity: &str,
        limit: &Limit,
    ) -> Result<serde_json::Value, PoolError> {
        let (period_start, _) = usage_tables::period_of((self.now)(), limit.period_secs.max(1));
        let used = durable_used(&self.control, space, quantity, period_start).await?;
        Ok(serde_json::json!({
            "space": space,
            "quantity": quantity,
            "limit": limit.limit,
            "grace": limit.grace,
            "periodSecs": limit.period_secs,
            "periodStart": period_start,
            "used": used,
            "enforced": self.enforced,
        }))
    }

    async fn read(
        &self,
        bound: &ordo_api::BoundParams,
    ) -> Result<Vec<serde_json::Value>, ordo_api::ExecError> {
        let want = bound
            .0
            .get("space")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let limits = all_limits(&self.control).await.map_err(unreadable)?;
        let mut rows = Vec::new();
        for ((space, quantity), limit) in &limits {
            if want.is_some_and(|w| w != space) {
                continue;
            }
            rows.push(self.row(space, quantity, limit).await.map_err(unreadable)?);
        }
        Ok(rows)
    }

    async fn set(
        &self,
        bound: &ordo_api::BoundParams,
        caller: &str,
    ) -> Result<serde_json::Value, ordo_api::ExecError> {
        let text = |k: &str| {
            bound
                .0
                .get(k)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or_default()
                .to_string()
        };
        let space = text("space");
        let quantity = text("quantity");
        let limit = Limit {
            limit: whole(bound, "limit")?,
            grace: whole(bound, "grace")?,
            period_secs: whole(bound, "periodSecs")?,
        };
        // A quota on a tenant that does not exist would look set and gate
        // nothing, as a misspelt quantity would. `ordo quota set` may still
        // name one ahead of time, at the database.
        if space != self.home {
            let known = crate::tenants::lookup(&self.control, &space)
                .await
                .map_err(unreadable)?;
            if known.is_none_or(|t| t.state == crate::tenants::State::Reaped) {
                return Err(ordo_api::ExecError::new(
                    ordo_api::ErrorCode::NotFound,
                    format!(
                        "no tenant `{space}` on this deployment: a quota is set on a tenant's \
                         id, as std.deployment.tenants lists it, or on the deployment's own \
                         space"
                    ),
                ));
            }
        }
        let result = set_limit(
            &self.control,
            &space,
            &quantity,
            limit.limit,
            limit.grace,
            limit.period_secs,
        )
        .await;
        // G12: the deployment owner's act leaves a record, stored or not.
        // In the deployment's own stream, mirrored to the tenant's.
        if let Some(stream) = self.stream.as_deref() {
            let outcome = match &result {
                Ok(()) => "set",
                Err(QuotaRefusal::Database(_)) => "failed",
                Err(_) => "refused",
            };
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act: "quota-set",
                    detail: format!(
                        "space={space} quantity={quantity} limit={} grace={} period={}s \
                         result={outcome}",
                        limit.limit, limit.grace, limit.period_secs
                    ),
                    by: Some(caller),
                    tenant: (space != self.home).then_some(space.as_str()),
                    deployment: true,
                    at: (self.now)(),
                },
            );
        }
        match result {
            Ok(()) => {}
            Err(QuotaRefusal::Database(e)) => {
                return Err(ordo_api::ExecError::new(
                    ordo_api::ErrorCode::SpaceUnavailable,
                    format!("the quota was not stored ({e}); nothing was changed"),
                ))
            }
            Err(refusal) => return Err(invalid(refusal.to_string())),
        }
        let mut row = self
            .row(&space, &quantity, &limit)
            .await
            .map_err(unreadable)?;
        if !self.enforced {
            row["note"] = serde_json::json!(format!(
                "stored, and gating nothing until {LEDGER_ENV}=true: quotas count from the usage \
                 ledger"
            ));
        }
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_api::{CostClass, NotSyncable, OpCost, OpKind, RefineTier, Syncability};

    fn op(name: &str) -> OpSpec {
        OpSpec {
            name: name.to_string(),
            version_hash: "v1".to_string(),
            kind: OpKind::Read,
            params: vec![],
            output_fields: vec![],
            live_capable: false,
            refinable: RefineTier::None,
            aggregate_only: false,
            mode_label: None,
            cost: OpCost::default(),
            sync: Syncability::NotSyncable(NotSyncable::NotARead),
            requires_idempotency_key: false,
            public: Default::default(),
        }
    }

    fn quotas_with(
        limits: &[((&str, &str), Limit)],
        used: &[((&str, &str), u64)],
        metrics: Arc<Metrics>,
    ) -> PgQuotas {
        let q = PgQuotas::new(metrics);
        {
            let mut l = q.limits.write().unwrap();
            for ((space, quantity), limit) in limits {
                l.insert((space.to_string(), quantity.to_string()), *limit);
            }
        }
        {
            let mut u = q.used.write().unwrap();
            for ((space, quantity), n) in used {
                u.insert((space.to_string(), quantity.to_string()), *n);
            }
        }
        q
    }

    #[test]
    fn no_row_is_always_within() {
        let q = PgQuotas::new(Arc::new(Metrics::new()));
        assert_eq!(q.check("spc_a", &op("x")), QuotaDecision::Within);
    }

    #[test]
    fn under_the_limit_is_within() {
        let q = quotas_with(
            &[(
                ("spc_a", QUANTITY_REQUESTS),
                Limit {
                    limit: 100,
                    grace: 10,
                    period_secs: 60,
                },
            )],
            &[(("spc_a", QUANTITY_REQUESTS), 50)],
            Arc::new(Metrics::new()),
        );
        assert_eq!(q.check("spc_a", &op("x")), QuotaDecision::Within);
    }

    #[test]
    fn over_the_limit_under_grace_warns() {
        let q = quotas_with(
            &[(
                ("spc_a", QUANTITY_REQUESTS),
                Limit {
                    limit: 100,
                    grace: 10,
                    period_secs: 60,
                },
            )],
            &[(("spc_a", QUANTITY_REQUESTS), 105)],
            Arc::new(Metrics::new()),
        );
        assert_eq!(
            q.check("spc_a", &op("x")),
            QuotaDecision::Warn {
                quantity: QUANTITY_REQUESTS,
                used: 105,
                limit: 100
            }
        );
    }

    #[test]
    fn over_limit_plus_grace_is_exceeded() {
        let q = quotas_with(
            &[(
                ("spc_a", QUANTITY_REQUESTS),
                Limit {
                    limit: 100,
                    grace: 10,
                    period_secs: 60,
                },
            )],
            &[(("spc_a", QUANTITY_REQUESTS), 111)],
            Arc::new(Metrics::new()),
        );
        assert_eq!(
            q.check("spc_a", &op("x")),
            QuotaDecision::Exceeded {
                quantity: QUANTITY_REQUESTS,
                used: 111,
                limit: 100,
                grace: 10
            }
        );
    }

    #[test]
    fn a_different_space_is_untouched() {
        let q = quotas_with(
            &[(
                ("spc_a", QUANTITY_REQUESTS),
                Limit {
                    limit: 1,
                    grace: 0,
                    period_secs: 60,
                },
            )],
            &[(("spc_a", QUANTITY_REQUESTS), 100)],
            Arc::new(Metrics::new()),
        );
        assert_eq!(q.check("spc_b", &op("x")), QuotaDecision::Within);
    }

    #[test]
    fn the_live_ledger_adds_to_the_durable_sum() {
        let metrics = Arc::new(Metrics::new().with_usage_ledger(true));
        metrics.usage_request("spc_a", CostClass::Sql, Some(1));
        metrics.usage_request("spc_a", CostClass::Sql, Some(1));
        let q = quotas_with(
            &[(
                ("spc_a", QUANTITY_REQUESTS),
                Limit {
                    limit: 1,
                    grace: 0,
                    period_secs: 60,
                },
            )],
            &[(("spc_a", QUANTITY_REQUESTS), 0)],
            metrics,
        );
        // 0 durable + 2 live == 2, which is >= limit(1) + grace(0).
        assert_eq!(
            q.check("spc_a", &op("x")),
            QuotaDecision::Exceeded {
                quantity: QUANTITY_REQUESTS,
                used: 2,
                limit: 1,
                grace: 0
            }
        );
    }

    #[test]
    fn set_limit_refuses_an_unknown_quantity() {
        let err = UnknownQuantity("writes".to_string());
        assert!(err.to_string().contains("writes"));
        assert!(err.to_string().contains("requests"));
    }
}
