//! The Usage screen's routes (`plan-console.md` 4.9 and 4.11, step 7).
//!
//! Every call reaches the deployment through the console's one client to
//! the door (`crate::door`, `crate::link`), with the console key, in the
//! deployment's own space:
//!
//! | Route | At the door |
//! |---|---|
//! | `GET usage?from&to[&tenant]` | `GET /usage` for the tenant, or for every tenant and the deployment's own space, with `std.deployment.tenants` for the list |
//! | `GET usage/quotas` | `std.deployment.quotas` |
//! | `POST usage/quotas/set` | `std.deployment.setQuota` |
//! | `GET usage/slow-ops` | `std.deployment.observedCost` |
//!
//! A quota change needs two touches, as every write at the deployment
//! does: the console owner's own and the deployment owner's, relayed.

use std::collections::BTreeMap;

use axum::extract::Query;

use super::*;

action!(set_quota_route, "usage.set_quota", set_quota);
read!(quotas_route, quotas_list);
read!(slow_ops_route, slow_ops);

const TENANTS: &str = "std.deployment.tenants";
const QUOTAS: &str = "std.deployment.quotas";
const SET_QUOTA: &str = "std.deployment.setQuota";
const OBSERVED_COST: &str = "std.deployment.observedCost";

/// The most tenants one deployment-wide reading asks the door about. Past
/// it the answer says how many it left out.
pub(crate) const MAX_TENANTS: usize = 200;
/// The longest range one reading covers: about 13 months.
const MAX_RANGE_SECS: u64 = 400 * 86_400;

/// The Usage routes, under `/d/<handle>/api/usage`.
pub(super) fn routes() -> Router<Console> {
    Router::new()
        .route("/d/{handle}/api/usage", get(usage_route))
        .route("/d/{handle}/api/usage/quotas", get(quotas_route))
        .route("/d/{handle}/api/usage/quotas/set", post(set_quota_route))
        .route("/d/{handle}/api/usage/slow-ops", get(slow_ops_route))
}

/// A space id as the door takes one: letters, digits, `_` and `-`.
fn space_id(field: &str, id: &str) -> Result<String, ConsoleError> {
    let id = id.trim();
    if id.is_empty()
        || id.len() > 128
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(ConsoleError::BadRequest(format!(
            "`{field}` is a tenant id: 1 to 128 letters, digits, `_` and `-`"
        )));
    }
    Ok(id.to_string())
}

/// `GET usage`: a read with a query, so it is spelled out rather than
/// made with `read!`.
async fn usage_route(
    State(c): State<Console>,
    Path(handle): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let mut call = Call {
        deployment: handle,
        actor: "anonymous".into(),
        detail: String::new(),
    };
    let result = match c.admit(&call, &headers, false) {
        Ok(()) => usage(&c, &mut call, &headers, &query).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(r) => r.into_response(),
        Err(e) => e.into_response(),
    }
}

/// One period's numbers, summed over every space read.
#[derive(Default, Clone)]
struct Period {
    end: u64,
    requests: u64,
    rows_returned: u64,
    writes: u64,
    rows_written: u64,
    bytes: u64,
}

impl Period {
    fn add(&mut self, record: &Value) {
        let n = |k: &str| record.get(k).and_then(Value::as_u64).unwrap_or(0);
        self.end = self.end.max(n("period_end"));
        self.requests += record
            .pointer("/totals/requests")
            .and_then(Value::as_object)
            .map(|m| m.values().filter_map(Value::as_u64).sum::<u64>())
            .unwrap_or(0);
        self.rows_returned += record
            .pointer("/totals/rows_returned")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        self.writes += n("writes");
        self.rows_written += n("rows_written");
        self.bytes +=
            n("bytes_sealed") + n("bytes_chain") + n("bytes_blob") + n("bytes_rows_estimated");
    }

    fn json(&self, start: Option<u64>) -> Value {
        let mut v = json!({
            "end": self.end,
            "requests": self.requests,
            "rowsReturned": self.rows_returned,
            "writes": self.writes,
            "rowsWritten": self.rows_written,
            "bytes": self.bytes,
        });
        if let Some(s) = start {
            v["start"] = json!(s);
        }
        v
    }
}

/// What one or more spaces' records add up to.
#[derive(Default)]
struct Reading {
    periods: BTreeMap<u64, Period>,
    gaps: Vec<Value>,
}

impl Reading {
    /// Fold one space's records in; answer that space's own total.
    fn add(&mut self, space: &str, records: &[Value]) -> Period {
        let mut own = Period::default();
        for r in records {
            let start = r.get("period_start").and_then(Value::as_u64).unwrap_or(0);
            if r.get("gap").and_then(Value::as_bool) == Some(true) {
                self.gaps.push(json!({
                    "space": space,
                    "start": start,
                    "end": r.get("period_end"),
                    "reason": r.get("reason"),
                }));
                continue;
            }
            self.periods.entry(start).or_default().add(r);
            own.add(r);
        }
        own
    }
}

/// Usage over a range: one tenant's, or the deployment's as a whole with
/// each tenant's share. A tenant the door would not answer for is named
/// with why, and the rest still shows.
async fn usage(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let secs = |k: &str| -> Result<u64, ConsoleError> {
        query
            .get(k)
            .and_then(|v| v.trim().parse::<u64>().ok())
            .ok_or_else(|| {
                ConsoleError::BadRequest(format!("`{k}` is a time in whole seconds since 1970"))
            })
    };
    let (from, to) = (secs("from")?, secs("to")?);
    if to <= from || to - from > MAX_RANGE_SECS {
        return Err(ConsoleError::BadRequest(
            "`to` comes after `from`, and the range is at most 400 days".into(),
        ));
    }
    let link = who.door(c)?;
    let tenant = query
        .get("tenant")
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| space_id("tenant", t))
        .transpose()?;

    let mut reading = Reading::default();
    if let Some(t) = &tenant {
        let records = link.usage(t, from, to).await?;
        reading.add(t, &records);
        return Ok(Reply::json(answer(
            "tenant",
            tenant.as_deref(),
            from,
            to,
            reading,
            Vec::new(),
            json!({}),
        )));
    }

    // The whole deployment: its own space, then each tenant that serves.
    let rows = link.op(TENANTS, true, &json!({})).await?;
    let home = Some(link.home_space().await?);
    let mut spaces: Vec<(String, String)> = Vec::new();
    if let Some(h) = home.clone() {
        spaces.push((h, "The deployment's own space".into()));
    }
    for r in &rows {
        if let (Some(id), Some(name)) = (
            r.get("id").and_then(Value::as_str),
            r.get("name").and_then(Value::as_str),
        ) {
            if !spaces.iter().any(|(s, _)| s == id) {
                spaces.push((id.to_string(), name.to_string()));
            }
        }
    }
    let left_out = spaces.len().saturating_sub(MAX_TENANTS);
    spaces.truncate(MAX_TENANTS);
    let mut by_tenant = Vec::new();
    let mut missing = Vec::new();
    for (id, name) in &spaces {
        match link.usage(id, from, to).await {
            Ok(records) => {
                let own = reading.add(id, &records);
                let mut row = own.json(None);
                row["id"] = json!(id);
                row["name"] = json!(name);
                by_tenant.push(row);
            }
            Err(e) => missing
                .push(json!({"id": id, "name": name, "error": e.code(), "message": e.to_string()})),
        }
    }
    Ok(Reply::json(answer(
        "deployment",
        None,
        from,
        to,
        reading,
        by_tenant,
        json!({"missing": missing, "leftOut": left_out, "home": home}),
    )))
}

fn answer(
    scope: &str,
    tenant: Option<&str>,
    from: u64,
    to: u64,
    reading: Reading,
    by_tenant: Vec<Value>,
    extra: Value,
) -> Value {
    let mut total = Period::default();
    let periods: Vec<Value> = reading
        .periods
        .iter()
        .map(|(start, p)| {
            total.requests += p.requests;
            total.rows_returned += p.rows_returned;
            total.writes += p.writes;
            total.rows_written += p.rows_written;
            total.bytes = total.bytes.max(p.bytes);
            p.json(Some(*start))
        })
        .collect();
    let mut v = json!({
        "scope": scope,
        "tenant": tenant,
        "from": from,
        "to": to,
        "periods": periods,
        "total": total.json(None),
        "byTenant": by_tenant,
        "gaps": reading.gaps,
        "missing": [],
        "leftOut": 0,
        // The deployment's own space, when the reading covers it.
        "home": null,
    });
    if let Value::Object(extra) = extra {
        for (k, x) in extra {
            v[k] = x;
        }
    }
    v
}

/// Every quota set at the deployment, with what was used of it.
async fn quotas_list(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let rows = who.door(c)?.op(QUOTAS, true, &json!({})).await?;
    Ok(Reply::json(json!({ "quotas": rows })))
}

#[derive(Deserialize)]
struct QuotaBody {
    space: String,
    quantity: String,
    limit: u64,
    grace: u64,
    #[serde(rename = "periodSecs")]
    period_secs: u64,
}

/// Set one space's quota for one quantity (`std.deployment.setQuota`).
/// The door checks the quantity and the tenant.
async fn set_quota(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: QuotaBody = parse(body)?;
    let space = space_id("space", &req.space)?;
    let quantity = req.quantity.trim().to_string();
    call.detail = format!(
        "space={space} quantity={quantity} limit={} grace={} period={}s",
        req.limit, req.grace, req.period_secs
    );
    let rows = who
        .door(c)?
        .op_present(
            SET_QUOTA,
            &json!({
                "space": space,
                "quantity": quantity,
                "limit": req.limit,
                "grace": req.grace,
                "periodSecs": req.period_secs,
            }),
        )
        .await?;
    Ok(Reply::json(json!({
        "quota": rows.into_iter().next().unwrap_or(Value::Null),
    })))
}

/// The ops this node served, with what they cost: calls, errors, mean and
/// longest time, rows, and the cost class each ran at. Per node, since its
/// start; each row says which node and since when.
async fn slow_ops(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let rows = who.door(c)?.op(OBSERVED_COST, true, &json!({})).await?;
    let node = rows.first().and_then(|r| r.get("node")).cloned();
    let since = rows.first().and_then(|r| r.get("since")).cloned();
    let ops: Vec<Value> = rows
        .into_iter()
        .map(|mut r| {
            let n = |k: &str| r.get(k).and_then(Value::as_u64).unwrap_or(0);
            let calls = n("calls");
            let mean = n("totalMicros").checked_div(calls).unwrap_or(0);
            // The class most of its calls ran at.
            let tier = r
                .get("effectiveClasses")
                .and_then(Value::as_object)
                .and_then(|m| {
                    m.iter()
                        .max_by_key(|(_, v)| v.as_u64().unwrap_or(0))
                        .map(|(k, _)| k.clone())
                })
                .or_else(|| {
                    r.get("registeredClass")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            r["meanMicros"] = json!(mean);
            r["tier"] = json!(tier);
            r
        })
        .collect();
    Ok(Reply::json(json!({
        "ops": ops,
        "node": node.unwrap_or(Value::Null),
        "since": since.unwrap_or(Value::Null),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_fold_into_periods_and_gaps() {
        let mut r = Reading::default();
        let own = r.add(
            "spc_a",
            &[
                json!({"period_start": 0, "period_end": 3600,
                       "totals": {"requests": {"sql": 3, "cache": 2}, "rows_returned": 9},
                       "writes": 4, "rows_written": 5, "bytes_sealed": 10, "bytes_chain": 1,
                       "bytes_blob": 0, "bytes_rows_estimated": 4}),
                json!({"period_start": 3600, "period_end": 7200, "gap": true,
                       "reason": "unreachable"}),
            ],
        );
        r.add(
            "spc_b",
            &[json!({"period_start": 0, "period_end": 3600,
                      "totals": {"requests": {"sql": 1}, "rows_returned": 1}})],
        );
        assert_eq!(own.requests, 5);
        assert_eq!(own.bytes, 15);
        let p = &r.periods[&0];
        assert_eq!(p.requests, 6);
        assert_eq!(p.rows_returned, 10);
        assert_eq!(p.writes, 4);
        assert_eq!(r.gaps.len(), 1);
        assert_eq!(r.gaps[0]["space"], "spc_a");
    }

    #[test]
    fn a_space_id_is_what_the_door_takes() {
        assert_eq!(space_id("space", " spc_a-1 ").unwrap(), "spc_a-1");
        for bad in ["", "a b", "a/b", &"x".repeat(129)] {
            assert!(space_id("space", bad).is_err(), "{bad}");
        }
    }
}
