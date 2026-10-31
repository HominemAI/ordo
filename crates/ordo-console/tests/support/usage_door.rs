//! A stand-in for the deployment's usage, quota and observed cost
//! answers, for the console's tests and its end-to-end stack
//! (`examples/e2e_door.rs` includes this file).
//!
//! The door's check stays the real one (`ordo_host::IdentityAccess`, with
//! its home space): this only answers once the door allowed the call, over
//! memory, in the shapes `ordo-host` answers (`GET /usage`,
//! `std.deployment.quotas`, `setQuota`, `observedCost`).
//! `ordo-host/tests/deployment_quotas.rs` covers the real quota ops against
//! Postgres, and `ordo-host/tests/usage.rs` the real `/usage`.

#![allow(dead_code)]

use std::sync::Mutex;

use ordo_identity::{AccessEngine, AccessProfile, Role};
use serde_json::{json, Value};

pub const QUOTAS: &str = "std.deployment.quotas";
pub const SET_QUOTA: &str = "std.deployment.setQuota";
pub const OBSERVED_COST: &str = "std.deployment.observedCost";
/// The op `GET /usage` is checked against, in the home space.
pub const USAGE_OP: &str = "platform.usage";

/// The ops this stand-in answers at `/op/{name}`.
pub const OPS: [&str; 3] = [QUOTAS, SET_QUOTA, OBSERVED_COST];

/// How many hourly periods each space has, ending at the last whole hour
/// before the range's end.
pub const PERIODS: u64 = 6;

/// Requests a period, by space. Spaces not named have none.
pub const REQUESTS: [(&str, u64); 3] = [("spc_mine", 10), ("spc_theirs", 5), ("home", 1)];
/// The space whose newest period is a gap.
pub const GAPPED: &str = "spc_held";

pub struct Usage {
    quotas: Mutex<Vec<Value>>,
    at: u64,
}

fn quota_row(space: &str, quantity: &str, limit: u64, grace: u64, period: u64, used: u64) -> Value {
    json!({
        "space": space, "quantity": quantity, "limit": limit, "grace": grace,
        "periodSecs": period, "periodStart": 0, "used": used, "enforced": true,
    })
}

impl Usage {
    pub fn new(at: u64) -> Usage {
        Usage {
            quotas: Mutex::new(vec![quota_row("spc_mine", "requests", 100, 10, 3600, 40)]),
            at,
        }
    }

    /// The owner's rules for these: the reads and `/usage` with no touch,
    /// `setQuota` with one.
    pub fn home_profile(profile: AccessProfile) -> AccessProfile {
        OPS.iter()
            .chain([USAGE_OP].iter())
            .fold(profile, |p, op| p.allow(Role::Owner, op))
    }

    pub fn grant(engine: &mut AccessEngine) {
        engine.require_step_up(SET_QUOTA);
    }

    pub fn quota_of(&self, space: &str, quantity: &str) -> Option<Value> {
        self.quotas
            .lock()
            .unwrap()
            .iter()
            .find(|q| q["space"] == space && q["quantity"] == quantity)
            .cloned()
    }

    /// `GET /usage?space&from&to`, one JSON record a line.
    pub fn usage(&self, space: &str, from: u64, to: u64) -> String {
        let per = REQUESTS
            .iter()
            .find(|(s, _)| *s == space)
            .map(|(_, n)| *n)
            .unwrap_or(0);
        let last = to - to % 3600;
        let mut out = String::new();
        for i in (1..=PERIODS).rev() {
            let start = last.saturating_sub(i * 3600);
            if start < from {
                continue;
            }
            let end = start + 3600;
            if space == GAPPED && i == 1 {
                out.push_str(&format!(
                    "{}\n",
                    json!({"schema": "ordo.usage/1", "space": space, "period_start": start,
                           "period_end": end, "gap": true, "target": "default",
                           "reason": "the tenant's database did not answer", "recorded_at": end})
                ));
                continue;
            }
            if per == 0 {
                continue;
            }
            out.push_str(&format!(
                "{}\n",
                json!({"schema": "ordo.usage/1", "space": space, "period_start": start,
                       "period_end": end, "method_version": "ordo.usage/1", "replicas": [],
                       "totals": {"requests": {"sql": per}, "rows_returned": per * 2,
                                  "worker_runs": 0},
                       "writes": per / 5, "rows_written": per / 5,
                       "bytes_sealed": 1000, "bytes_chain": 100, "bytes_blob": 0,
                       "bytes_rows_estimated": 900})
            ));
        }
        out
    }

    /// Answer one op the door allowed. `Err` is the door's code and message.
    pub fn execute(
        &self,
        name: &str,
        params: &serde_json::Map<String, Value>,
    ) -> Result<Vec<Value>, (String, String)> {
        let text = |k: &str| {
            params
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let num = |k: &str| {
            params.get(k).and_then(|v| match v {
                Value::Number(n) => n.as_u64(),
                Value::String(s) => s.parse().ok(),
                _ => None,
            })
        };
        match name {
            QUOTAS => Ok(self.quotas.lock().unwrap().clone()),
            SET_QUOTA => {
                let (space, quantity) = (text("space"), text("quantity"));
                if !["requests", "rows_returned"].contains(&quantity.as_str()) {
                    return Err((
                        "tenant_refused".into(),
                        format!(
                            "`{quantity}` is not a quota quantity: v1 gates requests and \
                             rows_returned"
                        ),
                    ));
                }
                let (Some(limit), Some(grace), Some(period)) =
                    (num("limit"), num("grace"), num("periodSecs"))
                else {
                    return Err(("tenant_refused".into(), "whole numbers".into()));
                };
                if period == 0 {
                    return Err((
                        "tenant_refused".into(),
                        "a quota's period is at least 1 second".into(),
                    ));
                }
                let row = quota_row(&space, &quantity, limit, grace, period, 0);
                let mut q = self.quotas.lock().unwrap();
                q.retain(|r| !(r["space"] == space.as_str() && r["quantity"] == quantity.as_str()));
                q.push(row.clone());
                Ok(vec![row])
            }
            OBSERVED_COST => {
                let row = |op: &str,
                           calls: u64,
                           total: u64,
                           max: u64,
                           errors: u64,
                           reg: &str,
                           classes: Value| {
                    json!({"op": op, "space": null, "calls": calls, "errors": errors,
                           "totalMicros": total, "maxMicros": max, "rowsReturned": calls * 3,
                           "registeredClass": reg, "effectiveClasses": classes,
                           "node": "node-a", "since": self.at - 7200})
                };
                Ok(vec![
                    row(
                        "getProfile",
                        400,
                        200_000,
                        3_000,
                        0,
                        "sql",
                        json!({"sql": 400}),
                    ),
                    row(
                        "listTasks",
                        120,
                        600_000,
                        90_000,
                        6,
                        "sql",
                        json!({"sql": 100, "blind-index": 20}),
                    ),
                    row(
                        "searchNotes",
                        15,
                        3_000_000,
                        800_000,
                        0,
                        "bounded-host",
                        json!({"bounded-host": 15}),
                    ),
                ])
            }
            other => Err(("not_found".into(), format!("no op {other}"))),
        }
    }
}
