//! Observed op cost (`plan-tiers.md`, "Build placement"; owner ruling
//! 2026-10-23, `plan-tiers-cheap`).
//!
//! Every execution through [`crate::metrics::MeteredExecutor`] records what
//! the op cost: calls, errors, total and longest duration, rows returned,
//! and the **effective cost class** the request ran at
//! (`api-engine-design.md` §7.1). Until this module, the door computed the
//! effective class and dropped it. Nothing reads these numbers to choose a
//! plan yet. They are collected now because telemetry that was never
//! collected cannot be backfilled when plan tiers arrive.
//!
//! # Bounded
//!
//! Two maps, each with a cap the deployment owner sets:
//!
//! - **Per op**, `ORDO_OBSERVED_COST_MAX_OPS`, default 4096. Op names come
//!   from the registry, because the door refuses an unknown op before the
//!   executor runs, so the population is the program's ops plus the
//!   platform's.
//! - **Per tenant and op**, `ORDO_OBSERVED_COST_MAX_SPACE_ENTRIES`, default
//!   10000. `0` keeps no per-tenant numbers.
//!
//! Past a cap a new entry is not made, and `ordo_op_observed_dropped_total`
//! counts the loss, so an incomplete table never reads as a quiet one.
//!
//! # Where it is read
//!
//! On `/metrics` ([`ObservedCosts::render_prometheus`]): the per-op
//! families always, the per-tenant families only when
//! `ORDO_METRICS_PER_SPACE` is on, for the cardinality reason
//! `crate::metrics` gives. And through `std.deployment.observedCost`, a read
//! for the deployment owner in the deployment's own space, which is what
//! the console calls.
//!
//! The numbers are per replica, in memory, and start again at a restart.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use serde_json::{json, Value};

/// `std.deployment.observedCost($space?, $bySpace?)`.
pub const OBSERVED_COST: &str = "std.deployment.observedCost";
/// Every op this module serves.
pub const OPS: [&str; 1] = [OBSERVED_COST];

/// The setting for the per-op cap.
pub const MAX_OPS_ENV: &str = "ORDO_OBSERVED_COST_MAX_OPS";
/// The setting for the per-tenant cap.
pub const MAX_SPACE_ENTRIES_ENV: &str = "ORDO_OBSERVED_COST_MAX_SPACE_ENTRIES";
/// The per-op cap when the setting is unset.
pub const DEFAULT_MAX_OPS: usize = 4096;
/// The per-tenant cap when the setting is unset.
pub const DEFAULT_MAX_SPACE_ENTRIES: usize = 10_000;

/// The caps. Read once, when the registry is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedCostConfig {
    pub max_ops: usize,
    pub max_space_entries: usize,
}

impl Default for ObservedCostConfig {
    fn default() -> Self {
        ObservedCostConfig {
            max_ops: DEFAULT_MAX_OPS,
            max_space_entries: DEFAULT_MAX_SPACE_ENTRIES,
        }
    }
}

impl ObservedCostConfig {
    /// The caps from the environment. A value that is not a whole number
    /// keeps the default and is reported on stderr naming the setting,
    /// because metrics must never stop a boot.
    pub fn from_env() -> ObservedCostConfig {
        let read = |var: &str, default: usize| match std::env::var(var) {
            Ok(v) => v.trim().parse::<usize>().unwrap_or_else(|_| {
                eprintln!("  {var}={v:?} is not a whole number; using {default}");
                default
            }),
            Err(_) => default,
        };
        ObservedCostConfig {
            max_ops: read(MAX_OPS_ENV, DEFAULT_MAX_OPS),
            max_space_entries: read(MAX_SPACE_ENTRIES_ENV, DEFAULT_MAX_SPACE_ENTRIES),
        }
    }
}

/// One op's observed cost since this replica started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservedCost {
    /// Executions, whatever their outcome.
    pub calls: u64,
    /// Executions that ended in a refusal from the executor.
    pub errors: u64,
    /// Summed wall time, in microseconds.
    pub total_micros: u64,
    /// The longest single execution, in microseconds.
    pub max_micros: u64,
    /// Rows the successful reads returned.
    pub rows_returned: u64,
    /// The class the op was registered at, as last seen.
    pub registered: &'static str,
    /// Executions by the effective cost class they ran at.
    pub by_class: BTreeMap<&'static str, u64>,
}

impl ObservedCost {
    fn add(&mut self, s: &Sample) {
        self.calls += 1;
        if !s.ok {
            self.errors += 1;
        }
        self.total_micros = self.total_micros.saturating_add(s.micros);
        self.max_micros = self.max_micros.max(s.micros);
        self.rows_returned = self.rows_returned.saturating_add(s.rows);
        self.registered = s.registered;
        *self.by_class.entry(s.effective).or_insert(0) += 1;
    }

    fn row(&self, op: &str, space: Option<&str>) -> Value {
        json!({
            "op": op,
            "space": space,
            "calls": self.calls,
            "errors": self.errors,
            "totalMicros": self.total_micros,
            "maxMicros": self.max_micros,
            "rowsReturned": self.rows_returned,
            "registeredClass": self.registered,
            "effectiveClasses": self.by_class,
        })
    }
}

/// One execution, as [`ObservedCosts::record`] takes it.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub micros: u64,
    pub rows: u64,
    pub ok: bool,
    pub registered: &'static str,
    pub effective: &'static str,
}

/// The two bounded maps. Held by [`crate::metrics::Metrics`].
pub struct ObservedCosts {
    config: ObservedCostConfig,
    per_op: Mutex<BTreeMap<String, ObservedCost>>,
    per_space: Mutex<BTreeMap<(String, String), ObservedCost>>,
    dropped: AtomicU64,
    /// When this replica began counting, in seconds since the epoch.
    since: u64,
}

fn unpoisoned<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl ObservedCosts {
    pub fn new(config: ObservedCostConfig) -> ObservedCosts {
        ObservedCosts {
            config,
            per_op: Mutex::new(BTreeMap::new()),
            per_space: Mutex::new(BTreeMap::new()),
            dropped: AtomicU64::new(0),
            since: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }

    pub fn config(&self) -> ObservedCostConfig {
        self.config
    }

    /// When this replica began counting, in seconds since the epoch: the
    /// numbers cover the time since.
    pub fn since(&self) -> u64 {
        self.since
    }

    /// Record one execution of `op` in `space`.
    pub fn record(&self, op: &str, space: &str, s: Sample) {
        {
            let mut m = unpoisoned(&self.per_op);
            if let Some(c) = m.get_mut(op) {
                c.add(&s);
            } else if m.len() < self.config.max_ops {
                m.entry(op.to_string()).or_default().add(&s);
            } else {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        if self.config.max_space_entries == 0 {
            return;
        }
        let mut m = unpoisoned(&self.per_space);
        let key = (space.to_string(), op.to_string());
        if let Some(c) = m.get_mut(&key) {
            c.add(&s);
        } else if m.len() < self.config.max_space_entries {
            m.entry(key).or_default().add(&s);
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Every op's numbers, op-name sorted.
    pub fn per_op(&self) -> Vec<(String, ObservedCost)> {
        unpoisoned(&self.per_op)
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Every (tenant, op) entry, or one tenant's, sorted.
    pub fn per_space(&self, space: Option<&str>) -> Vec<(String, String, ObservedCost)> {
        unpoisoned(&self.per_space)
            .iter()
            .filter(|((s, _), _)| space.is_none_or(|want| want == s))
            .map(|((s, o), v)| (s.clone(), o.clone(), v.clone()))
            .collect()
    }

    /// Entries a cap refused.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// The Prometheus families. Per-tenant ones only when `space_labels`.
    pub fn render_prometheus(&self, out: &mut String, space_labels: bool) {
        let ops = self.per_op();
        render_families(
            out,
            "ordo_op_observed",
            ops.iter().map(|(o, c)| (None, o, c)),
        );
        if space_labels {
            let spaces = self.per_space(None);
            render_families(
                out,
                "ordo_space_op_observed",
                spaces.iter().map(|(s, o, c)| (Some(s.as_str()), o, c)),
            );
        }
        out.push_str("# TYPE ordo_op_observed_dropped_total counter\n");
        out.push_str(&format!(
            "ordo_op_observed_dropped_total {}\n",
            self.dropped()
        ));
    }
}

fn render_families<'a>(
    out: &mut String,
    prefix: &str,
    rows: impl Iterator<Item = (Option<&'a str>, &'a String, &'a ObservedCost)> + Clone,
) {
    let labels = |space: Option<&str>, op: &str, extra: Option<(&str, &str)>| {
        let mut l = Vec::new();
        if let Some(s) = space {
            l.push(format!("space=\"{}\"", escape(s)));
        }
        l.push(format!("op=\"{}\"", escape(op)));
        if let Some((k, v)) = extra {
            l.push(format!("{k}=\"{}\"", escape(v)));
        }
        format!("{{{}}}", l.join(","))
    };
    let mut family = |name: &str, kind: &str, value: &dyn Fn(&ObservedCost) -> String| {
        let mut any = false;
        for (space, op, c) in rows.clone() {
            if !any {
                out.push_str(&format!("# TYPE {prefix}_{name} {kind}\n"));
                any = true;
            }
            out.push_str(&format!(
                "{prefix}_{name}{} {}\n",
                labels(space, op, None),
                value(c)
            ));
        }
    };
    family("errors_total", "counter", &|c| c.errors.to_string());
    family("seconds_total", "counter", &|c| {
        format!("{}", c.total_micros as f64 / 1e6)
    });
    family("seconds_max", "gauge", &|c| {
        format!("{}", c.max_micros as f64 / 1e6)
    });
    family("rows_total", "counter", &|c| c.rows_returned.to_string());
    // Calls carry the effective class, a closed set, so the family is still
    // bounded by ops times classes.
    let mut any = false;
    for (space, op, c) in rows {
        for (class, n) in &c.by_class {
            if !any {
                out.push_str(&format!("# TYPE {prefix}_calls_total counter\n"));
                any = true;
            }
            out.push_str(&format!(
                "{prefix}_calls_total{} {n}\n",
                labels(space, op, Some(("class", class)))
            ));
        }
    }
}

fn escape(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// The op's spec. A read with two optional parameters: `space`, one tenant,
/// and `bySpace`, every tenant's entries rather than the per-op totals.
pub fn specs() -> Vec<OpSpec> {
    vec![OpSpec {
        name: OBSERVED_COST.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind: OpKind::Read,
        params: vec![
            ParamSpec {
                name: "space".to_string(),
                ty: ParamTy::Text { max_len: Some(256) },
                required: false,
                bounds: None,
            },
            ParamSpec {
                name: "bySpace".to_string(),
                ty: ParamTy::Bool,
                required: false,
                bounds: None,
            },
        ],
        output_fields: [
            "op",
            "space",
            "calls",
            "errors",
            "totalMicros",
            "maxMicros",
            "rowsReturned",
            "registeredClass",
            "effectiveClasses",
            "node",
            "since",
        ]
        .iter()
        .map(|f| FieldSpec::new(*f))
        .collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }]
}

/// What the op needs: the registry the numbers live in and the
/// deployment's own space, the only one it answers in.
pub struct ObservedCostOps {
    pub costs: Arc<crate::metrics::Metrics>,
    pub home: String,
    /// This replica's name (`ORDO_REPLICA_ID`), so a reader knows which
    /// node's numbers it holds (`plan-console.md` 4.11).
    pub node: String,
}

impl ObservedCostOps {
    /// `std.deployment.observedCost`. Without parameters it answers one row
    /// per op. With `space`, or `bySpace: true`, one row per (tenant, op).
    pub fn execute(&self, space: &str, bound: &BoundParams) -> Result<ExecResult, ExecError> {
        if space != self.home {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "observed op cost is the deployment's and this op answers only in the \
                 deployment's own space",
            ));
        }
        let costs = self.costs.observed_cost();
        let want = bound
            .0
            .get("space")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let by_space = bound
            .0
            .get("bySpace")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut rows: Vec<Value> = if want.is_some() || by_space {
            costs
                .per_space(want)
                .iter()
                .map(|(s, o, c)| c.row(o, Some(s)))
                .collect()
        } else {
            costs.per_op().iter().map(|(o, c)| c.row(o, None)).collect()
        };
        // Each row names the node that counted it and since when: the
        // numbers are per replica and start again at a restart.
        for r in &mut rows {
            r["node"] = json!(self.node);
            r["since"] = json!(costs.since());
        }
        Ok(ExecResult {
            rows,
            next_cursor: None,
            position: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(micros: u64, rows: u64, ok: bool, effective: &'static str) -> Sample {
        Sample {
            micros,
            rows,
            ok,
            registered: "sql",
            effective,
        }
    }

    #[test]
    fn observed_cost_accumulates_and_reads_back() {
        let c = ObservedCosts::new(ObservedCostConfig::default());
        c.record("listTasks", "t1", sample(1_000, 3, true, "sql"));
        c.record("listTasks", "t1", sample(5_000, 2, true, "blind-index"));
        c.record("listTasks", "t2", sample(2_000, 0, false, "sql"));

        let ops = c.per_op();
        assert_eq!(ops.len(), 1);
        let (op, got) = &ops[0];
        assert_eq!(op, "listTasks");
        assert_eq!(got.calls, 3);
        assert_eq!(got.errors, 1);
        assert_eq!(got.total_micros, 8_000);
        assert_eq!(got.max_micros, 5_000);
        assert_eq!(got.rows_returned, 5);
        assert_eq!(got.registered, "sql");
        assert_eq!(got.by_class.get("sql"), Some(&2));
        assert_eq!(got.by_class.get("blind-index"), Some(&1));

        let t1 = c.per_space(Some("t1"));
        assert_eq!(t1.len(), 1);
        assert_eq!(t1[0].2.calls, 2);
        assert_eq!(c.per_space(None).len(), 2);
    }

    #[test]
    fn the_caps_hold_and_the_loss_is_counted() {
        let c = ObservedCosts::new(ObservedCostConfig {
            max_ops: 1,
            max_space_entries: 0,
        });
        c.record("a", "t1", sample(1, 0, true, "sql"));
        c.record("b", "t1", sample(1, 0, true, "sql"));
        assert_eq!(c.per_op().len(), 1);
        assert!(
            c.per_space(None).is_empty(),
            "0 keeps no per-tenant numbers"
        );
        assert_eq!(c.dropped(), 1);
    }

    /// An executor that answers every call with two rows.
    struct TwoRows;

    impl ordo_api::OpExecutor for TwoRows {
        fn execute<'a>(
            &'a self,
            _op: &'a OpSpec,
            _space: &'a str,
            _bound: BoundParams,
            _refine: Option<ordo_api::Refinement>,
            _page: Option<ordo_api::PageCursor>,
            _lease: Option<ordo_api::ReplayLease>,
        ) -> futures::future::BoxFuture<'a, Result<ordo_api::ExecOutcome, ExecError>> {
            Box::pin(async {
                Ok(ordo_api::ExecOutcome::Ran(ExecResult {
                    rows: vec![json!({}), json!({})],
                    next_cursor: None,
                    position: None,
                }))
            })
        }
    }

    /// Usage metering takes the class the request ran at (owner ruling,
    /// 2026-10-23), not the class the op was registered at. The effective
    /// class is a join over the registered one, so it differs only when a
    /// refinement raises it: the request is metered at that class.
    #[tokio::test]
    async fn usage_metering_counts_the_effective_class_not_the_registered_one() {
        use ordo_api::OpExecutor;
        let metrics = Arc::new(
            crate::metrics::Metrics::new()
                .with_per_space_series(false)
                .with_usage_ledger(true),
        );
        let exec = crate::metrics::MeteredExecutor::new(Arc::new(TwoRows), metrics.clone());
        let mut op = specs().remove(0);
        op.name = "listTasks".into();
        let mut sealed = FieldSpec::new("title");
        sealed.filter = true;
        sealed.refine_class = ordo_api::CostClass::BlindIndex;
        op.output_fields.push(sealed);
        let refine = ordo_api::Refinement {
            where_: vec![ordo_api::RefineCond {
                field: "title".into(),
                op: ordo_api::RefineOp::Eq,
                value: json!("x"),
            }],
            pick: None,
        };
        exec.execute(&op, "t1", BoundParams::default(), None, None, None)
            .await
            .unwrap();
        exec.execute(&op, "t1", BoundParams::default(), Some(refine), None, None)
            .await
            .unwrap();
        let taken = metrics.usage_take();
        let t1 = taken.get("t1").expect("t1 metered");
        assert_eq!(t1.requests.get("sql"), Some(&1));
        assert_eq!(t1.requests.get("blind-index"), Some(&1));
        assert_eq!(t1.rows_returned, 4);
    }

    /// Through the metered executor: the effective class a refinement
    /// raises the request to is the one recorded, the rows are counted, the
    /// numbers reach `/metrics`, and the owner's op reads them back.
    #[tokio::test]
    async fn the_metered_executor_records_the_effective_class_and_the_op_reads_it() {
        use ordo_api::OpExecutor;
        let metrics = Arc::new(crate::metrics::Metrics::new().with_per_space_series(false));
        let exec = crate::metrics::MeteredExecutor::new(Arc::new(TwoRows), metrics.clone());

        let mut op = specs().remove(0);
        op.name = "listTasks".into();
        let mut sealed = FieldSpec::new("title");
        sealed.filter = true;
        sealed.refine_class = ordo_api::CostClass::BlindIndex;
        op.output_fields.push(sealed);
        let refine = ordo_api::Refinement {
            where_: vec![ordo_api::RefineCond {
                field: "title".into(),
                op: ordo_api::RefineOp::Eq,
                value: json!("x"),
            }],
            pick: None,
        };

        exec.execute(&op, "t1", BoundParams::default(), None, None, None)
            .await
            .unwrap();
        exec.execute(&op, "t1", BoundParams::default(), Some(refine), None, None)
            .await
            .unwrap();

        let ops = metrics.observed_cost().per_op();
        let (_, got) = ops.iter().find(|(o, _)| o == "listTasks").unwrap();
        assert_eq!(got.calls, 2);
        assert_eq!(got.rows_returned, 4);
        assert_eq!(got.registered, "sql");
        assert_eq!(got.by_class.get("sql"), Some(&1));
        assert_eq!(got.by_class.get("blind-index"), Some(&1));

        let text = metrics.render_prometheus();
        assert!(
            text.contains("ordo_op_observed_calls_total{op=\"listTasks\",class=\"blind-index\"} 1"),
            "{text}"
        );
        assert!(text.contains("ordo_op_observed_rows_total{op=\"listTasks\"} 4"));

        let read = ObservedCostOps {
            costs: metrics.clone(),
            home: "home".into(),
            node: "node-a".into(),
        };
        assert!(read.execute("t1", &BoundParams::default()).is_err());
        let rows = read.execute("home", &BoundParams::default()).unwrap().rows;
        assert_eq!(rows[0]["op"], "listTasks");
        assert_eq!(rows[0]["node"], "node-a");
        assert!(rows[0]["since"].as_u64().unwrap() > 0);
        assert_eq!(rows[0]["calls"], 2);
        assert_eq!(rows[0]["effectiveClasses"]["blind-index"], 1);
        let mut by_space = BoundParams::default();
        by_space.0.insert("space".into(), json!("t1"));
        let rows = read.execute("home", &by_space).unwrap().rows;
        assert_eq!(rows[0]["space"], "t1");
        assert_eq!(rows[0]["rowsReturned"], 4);
    }

    #[test]
    fn per_tenant_series_render_only_with_per_space_labels() {
        let c = ObservedCosts::new(ObservedCostConfig::default());
        c.record("listTasks", "t1", sample(2_500, 4, true, "sql"));
        let mut off = String::new();
        c.render_prometheus(&mut off, false);
        assert!(off.contains("ordo_op_observed_calls_total{op=\"listTasks\",class=\"sql\"} 1"));
        assert!(off.contains("ordo_op_observed_seconds_total{op=\"listTasks\"} 0.0025"));
        assert!(off.contains("ordo_op_observed_seconds_max{op=\"listTasks\"} 0.0025"));
        assert!(off.contains("ordo_op_observed_rows_total{op=\"listTasks\"} 4"));
        assert!(off.contains("ordo_op_observed_dropped_total 0"));
        assert!(!off.contains("space="), "{off}");

        let mut on = String::new();
        c.render_prometheus(&mut on, true);
        assert!(on.contains(
            "ordo_space_op_observed_calls_total{space=\"t1\",op=\"listTasks\",class=\"sql\"} 1"
        ));
    }
}
