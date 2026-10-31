//! Reading and keeping function runs (`plan-backend-functions.md` §10).
//!
//! - **What a run holds.** One `$fnruns` row per call
//!   ([`crate::functions::record_run`]): the function, its version, the
//!   tenant, who it ran as, when, how long, the outcome, and the log lines
//!   the function wrote with `console.*`. Each line is scrubbed of every live
//!   secret value (shown as `[secret]`) and the log is capped per call
//!   ([`ordo_fn::Limits::log_bytes`], 64 KB by default, marked when cut).
//!   Never the input, the answer or a body, unless the function logged them
//!   itself.
//! - **Who reads them.** The deployment owner, in the deployment's own
//!   space: [`RUNS`] lists runs newest first, [`RUN`] answers one run with
//!   its log. Both are reads, so `GET`. A tenant's own view of its runs is a
//!   later platform op, closed by default (§10).
//! - **How long they are kept.** [`RETENTION_ENV`] days, 14 by default. The
//!   supervised worker [`FnRunSweep`] deletes older runs, their logs with
//!   them, in batches of [`SWEEP_BATCH`].

use std::time::Duration;

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_exec::{sql_lit, Pool};
use serde_json::{json, Value};

use crate::supervisor::{BoxFut, Cadence, Progress, Worker, WorkerCtx, WorkerError};

/// `std.deployment.functionRuns($function?, $tenant?, $limit?)`.
pub const RUNS: &str = "std.deployment.functionRuns";
/// `std.deployment.functionRun($seq)`: one run and its log.
pub const RUN: &str = "std.deployment.functionRun";
/// Every op this module serves.
pub const OPS: [&str; 2] = [RUNS, RUN];

/// Runs one page answers when `limit` is not given.
pub const PAGE_DEFAULT: u64 = 50;
/// The most runs one page answers.
pub const PAGE_MAX: u64 = 500;

/// Days a run and its log are kept.
pub const RETENTION_ENV: &str = "ORDO_FN_RUN_RETENTION_DAYS";
pub const DEFAULT_RETENTION_DAYS: u64 = 14;
/// The longest a deployment may keep runs: ten years.
pub const MAX_RETENTION_DAYS: u64 = 3650;
const DAY_SECS: u64 = 24 * 60 * 60;

/// Runs one sweep pass deletes. The rest are the next pass's, which
/// `Progress::more` asks for at once.
pub const SWEEP_BATCH: u64 = 1000;

/// [`RETENTION_ENV`] read from a value: unset or empty is the default, 1 to
/// [`MAX_RETENTION_DAYS`] is that many days, anything else is refused with
/// the reason.
pub fn retention_from(raw: Option<&str>) -> Result<u64, String> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(DEFAULT_RETENTION_DAYS);
    };
    match raw.parse::<u64>() {
        Ok(n) if (1..=MAX_RETENTION_DAYS).contains(&n) => Ok(n),
        _ => Err(format!(
            "`{raw}` is not a number of days from 1 to {MAX_RETENTION_DAYS}; \
             unset, function runs are kept {DEFAULT_RETENTION_DAYS} days"
        )),
    }
}

/// [`retention_from`] over the process's environment.
pub fn retention_from_env() -> Result<u64, String> {
    retention_from(std::env::var(RETENTION_ENV).ok().as_deref())
}

/// One pass: delete up to [`SWEEP_BATCH`] runs that started more than
/// `keep_days` before `now` (seconds), oldest first. Answers how many went.
/// A deployment that never ran a function has no `$fnruns` and nothing to
/// do.
pub async fn sweep(control: &Pool, keep_days: u64, now: u64) -> Result<u64, String> {
    let Some(cutoff) = now.checked_sub(keep_days.saturating_mul(DAY_SECS)) else {
        return Ok(0);
    };
    if !crate::registrations::table_exists(control, "$fnruns").await? {
        return Ok(0);
    }
    let rows = control
        .rows(&format!(
            "DELETE FROM \"$fnruns\" WHERE \"seq\" IN ( \
               SELECT \"seq\" FROM \"$fnruns\" WHERE \"started_at\" < {cutoff} \
               ORDER BY \"seq\" LIMIT {SWEEP_BATCH}) \
             RETURNING \"seq\""
        ))
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows.len() as u64)
}

/// The supervised worker that keeps runs [`RETENTION_ENV`] days. **One
/// replica**, as the other sweeps: two would delete the same rows, which is
/// harmless and wasted.
pub struct FnRunSweep {
    pub keep_days: u64,
    pub every: Duration,
}

impl FnRunSweep {
    pub const NAME: &'static str = "functions.runs.sweep";
    /// Once an hour: a run is kept to within an hour of its day.
    pub const DEFAULT_SECS: u64 = 3600;
}

impl Worker for FnRunSweep {
    fn name(&self) -> &'static str {
        FnRunSweep::NAME
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.every)
    }
    fn requires_leadership(&self) -> bool {
        true
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let n = sweep(&ctx.pool, self.keep_days, ctx.now())
                .await
                .map_err(WorkerError)?;
            Ok(Progress {
                units: n,
                more: n == SWEEP_BATCH,
            })
        })
    }
}

fn param(name: &str, ty: ParamTy, required: bool) -> ParamSpec {
    ParamSpec {
        name: name.to_string(),
        ty,
        required,
        bounds: None,
    }
}

const LIST_FIELDS: [&str; 9] = [
    "seq",
    "function",
    "version",
    "tenant",
    "wake",
    "principal",
    "startedAt",
    "wallMs",
    "outcome",
];

const ONE_FIELDS: [&str; 9] = [
    "cpuMs",
    "peakMemory",
    "fetches",
    "hosts",
    "bytesOut",
    "bytesIn",
    "message",
    "logs",
    "logLines",
];

/// The two ops' specs: reads, so `GET`.
pub fn specs() -> Vec<OpSpec> {
    let spec = |name: &str, params: Vec<ParamSpec>, fields: Vec<&str>| OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind: OpKind::Read,
        params,
        output_fields: fields.into_iter().map(FieldSpec::new).collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    let text = || ParamTy::Text { max_len: Some(128) };
    let mut list = LIST_FIELDS.to_vec();
    list.push("logLines");
    let mut one = LIST_FIELDS.to_vec();
    one.extend(ONE_FIELDS);
    vec![
        spec(
            RUNS,
            vec![
                param("function", text(), false),
                param("tenant", text(), false),
                param("limit", ParamTy::Int, false),
            ],
            list,
        ),
        spec(RUN, vec![param("seq", ParamTy::Int, true)], one),
    ]
}

fn refused(msg: impl Into<String>) -> ExecError {
    ExecError::new(ErrorCode::TenantRefused, msg)
}

fn whole(bound: &BoundParams, name: &str) -> Result<Option<u64>, ExecError> {
    let bad = || refused(format!("`{name}` is a whole number, 1 or more"));
    match bound.0.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n.as_u64().map(Some).ok_or_else(bad),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => s.trim().parse().map(Some).map_err(|_| bad()),
        Some(_) => Err(bad()),
    }
}

fn word<'a>(bound: &'a BoundParams, name: &str) -> Option<&'a str> {
    bound
        .0
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// What the two ops need.
pub struct FnRunOps {
    /// The control database, which holds `$fnruns`.
    pub control: Pool,
    /// The deployment's own space, the only one the ops answer in.
    pub home: String,
}

const COLUMNS: &str = "\"seq\", \"function\", \"version\", \"space\", \"wake\", \"principal\", \
    \"started_at\", \"wall_ms\", \"outcome\", \"logs\", \"cpu_ms\", \"peak_memory\", \
    \"fetches\", \"hosts\", \"bytes_out\", \"bytes_in\", \"message\"";

/// One row of [`COLUMNS`] as the ops answer it. `full` adds the log and the
/// measures [`RUN`] answers.
fn answer(r: &[Option<String>], full: bool) -> Value {
    let cell = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
    let num = |i: usize| cell(i).parse::<u64>().unwrap_or(0);
    let logs: Vec<Value> = serde_json::from_str(&cell(9)).unwrap_or_default();
    let level = |l: &Value| match l["level"].as_u64() {
        Some(0) => "debug",
        Some(2) => "warn",
        Some(3) => "error",
        _ => "info",
    };
    let mut row = json!({
        "seq": num(0),
        "function": cell(1),
        "version": cell(2),
        "tenant": cell(3),
        "wake": cell(4),
        "principal": cell(5),
        "startedAt": num(6),
        "wallMs": num(7),
        "outcome": cell(8),
        "logLines": logs.len(),
    });
    if full {
        let o = row.as_object_mut().expect("an object");
        o.insert("cpuMs".into(), json!(num(10)));
        o.insert("peakMemory".into(), json!(num(11)));
        o.insert("fetches".into(), json!(num(12)));
        o.insert(
            "hosts".into(),
            json!(cell(13)
                .split(',')
                .filter(|h| !h.is_empty())
                .collect::<Vec<_>>()),
        );
        o.insert("bytesOut".into(), json!(num(14)));
        o.insert("bytesIn".into(), json!(num(15)));
        o.insert("message".into(), json!(cell(16)));
        o.insert(
            "logs".into(),
            Value::Array(
                logs.iter()
                    .map(|l| json!({"level": level(l), "text": l["text"]}))
                    .collect(),
            ),
        );
    }
    row
}

impl FnRunOps {
    /// Run [`RUNS`] or [`RUN`] in `space`.
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
    ) -> Result<ExecResult, ExecError> {
        if space != self.home {
            // `not_found`, as every deployment op answers outside its home.
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "function runs are the deployment's: these ops answer only in the deployment's \
                 own space",
            ));
        }
        let exists = crate::registrations::table_exists(&self.control, "$fnruns")
            .await
            .map_err(ExecError::internal)?;
        let rows = match op {
            RUNS => {
                let limit = match whole(bound, "limit")? {
                    None => PAGE_DEFAULT,
                    Some(0) => return Err(refused("`limit` is 1 or more")),
                    Some(n) => n.min(PAGE_MAX),
                };
                let mut filter = Vec::new();
                if let Some(f) = word(bound, "function") {
                    filter.push(format!("\"function\" = {}", sql_lit(f)));
                }
                if let Some(t) = word(bound, "tenant") {
                    filter.push(format!("\"space\" = {}", sql_lit(t)));
                }
                let filter = if filter.is_empty() {
                    String::new()
                } else {
                    format!("WHERE {}", filter.join(" AND "))
                };
                if !exists {
                    Vec::new()
                } else {
                    self.control
                        .rows(&format!(
                            "SELECT {COLUMNS} FROM \"$fnruns\" {filter} \
                             ORDER BY \"seq\" DESC LIMIT {limit}"
                        ))
                        .await
                        .map_err(|e| ExecError::internal(e.to_string()))?
                        .iter()
                        .map(|r| answer(r, false))
                        .collect()
                }
            }
            RUN => {
                let Some(seq) = whole(bound, "seq")? else {
                    return Err(refused("`seq` names the run, as functionRuns lists it"));
                };
                let found = if exists {
                    self.control
                        .rows(&format!(
                            "SELECT {COLUMNS} FROM \"$fnruns\" WHERE \"seq\" = {seq}"
                        ))
                        .await
                        .map_err(|e| ExecError::internal(e.to_string()))?
                } else {
                    Vec::new()
                };
                let Some(r) = found.first() else {
                    return Err(ExecError::new(
                        ErrorCode::NotFound,
                        format!(
                            "no function run {seq}: runs are kept {RETENTION_ENV} days \
                             ({DEFAULT_RETENTION_DAYS} by default), and {RUNS} lists those kept"
                        ),
                    ));
                };
                vec![answer(r, true)]
            }
            other => {
                return Err(ExecError::internal(format!(
                    "`{other}` is not a function-run op"
                )))
            }
        };
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

    #[test]
    fn retention_is_14_days_unset_and_a_bad_value_names_the_range() {
        assert_eq!(retention_from(None), Ok(14));
        assert_eq!(retention_from(Some(" ")), Ok(14));
        assert_eq!(retention_from(Some("30")), Ok(30));
        for bad in ["0", "-1", "two", "3651"] {
            let e = retention_from(Some(bad)).unwrap_err();
            assert!(e.contains("1 to 3650"), "{e}");
        }
    }

    #[test]
    fn a_listed_run_has_no_log_text_and_one_run_has_it_with_named_levels() {
        let logs = r#"[{"level":1,"text":"a"},{"level":3,"text":"b [secret]"}]"#;
        let r: Vec<Option<String>> = [
            "7", "f", "v1", "acme", "call", "p", "100", "5", "ok", logs, "1", "2", "0", "", "0",
            "0", "",
        ]
        .iter()
        .map(|s| Some(s.to_string()))
        .collect();
        let short = answer(&r, false);
        assert_eq!(short["logLines"], 2);
        assert!(short.get("logs").is_none());
        let full = answer(&r, true);
        assert_eq!(
            full["logs"][1],
            json!({"level": "error", "text": "b [secret]"})
        );
        assert_eq!(full["hosts"], json!([]));
    }
}
