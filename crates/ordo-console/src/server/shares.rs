//! The Shares and links screen and the Security log screen
//! (`plan-console.md` 4.7 and 4.8, step 9).
//!
//! Every call to the deployment goes through the console's one client to
//! the door (`crate::door`, `crate::link`):
//!
//! | Route | At the door | Where |
//! |---|---|---|
//! | `GET shares?tenant` | `std.space.links`, `std.space.shares` | the tenant, with a touch |
//! | `POST shares/revoke-link` | `std.space.revokeLink` | the tenant, both touches |
//! | `POST shares/revoke-share` | `std.space.revokeShare` | the tenant, both touches |
//! | `GET security?source=deployment` | `std.deployment.securityEvents` (G8) | home space |
//! | `GET security?source=tenant&tenant` | `GET /security/events?space=` | the tenant, with a touch |
//! | `GET security?source=console` | the console's own audit log | the console's database |
//!
//! A console session leaves the deployment's own space only with the
//! deployment owner's touch in the last 5 minutes, reads included, and the
//! door still asks whether the owner holds a role in the tenant that
//! reaches the op. A tenant the owner was given no role in stays closed:
//! ruled 2026-10-23 (ask 5), the owner sees a tenant's name and numbers,
//! not its shares.

use axum::extract::Query;

use super::*;

action!(revoke_link_route, "shares.revoke_link", revoke_link);
action!(revoke_share_route, "shares.revoke_share", revoke_share);

const LINKS: &str = "std.space.links";
const SHARES: &str = "std.space.shares";
const REVOKE_LINK: &str = "std.space.revokeLink";
const REVOKE_SHARE: &str = "std.space.revokeShare";
const EVENTS: &str = "std.deployment.securityEvents";

/// The most rows one page of a security log answers.
const PAGE_MAX: u64 = 200;
/// Rows a page answers when `limit` is not given.
const PAGE_DEFAULT: u64 = 50;

/// The routes, under `/d/<handle>/api/shares` and `/d/<handle>/api/security`.
pub(super) fn routes() -> Router<Console> {
    Router::new()
        .route("/d/{handle}/api/shares", get(shares_route))
        .route(
            "/d/{handle}/api/shares/revoke-link",
            post(revoke_link_route),
        )
        .route(
            "/d/{handle}/api/shares/revoke-share",
            post(revoke_share_route),
        )
        .route("/d/{handle}/api/security", get(security_route))
}

/// An id as the door takes one: letters, digits, `_` and `-`.
fn id_of(field: &str, id: &str) -> Result<String, ConsoleError> {
    let id = id.trim();
    if id.is_empty()
        || id.len() > 128
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(ConsoleError::BadRequest(format!(
            "`{field}` is an id: 1 to 128 letters, digits, `_` and `-`"
        )));
    }
    Ok(id.to_string())
}

/// A whole number from the query, if given.
fn whole(query: &HashMap<String, String>, k: &str) -> Result<Option<u64>, ConsoleError> {
    match query.get(k).map(|v| v.trim()).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) => v
            .parse::<u64>()
            .map(Some)
            .map_err(|_| ConsoleError::BadRequest(format!("`{k}` is a whole number, 0 or more"))),
    }
}

/// A GET with a query, spelled out rather than made with `read!`.
macro_rules! query_read {
    ($name:ident, $inner:ident) => {
        async fn $name(
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
                Ok(()) => $inner(&c, &mut call, &headers, &query).await,
                Err(e) => Err(e),
            };
            match result {
                Ok(r) => r.into_response(),
                Err(e) => e.into_response(),
            }
        }
    };
}

query_read!(shares_route, shares_list);
query_read!(security_route, security_page);

/// A tenant's links, each with its state (`live`, `expired`, `used-up`,
/// `revoked`), opens, devices and any lock, and its live shares.
async fn shares_list(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let tenant = id_of(
        "tenant",
        query.get("tenant").map(String::as_str).unwrap_or(""),
    )?;
    let link = who.door(c)?;
    let links = link.read_present_in(&tenant, LINKS, &json!({})).await?;
    let shares = link.read_present_in(&tenant, SHARES, &json!({})).await?;
    let links: Vec<Value> = links.into_iter().map(link_row).collect();
    Ok(Reply::json(json!({
        "tenant": tenant,
        "links": links,
        "shares": shares,
    })))
}

/// One link as the screen shows it. `devices` is the door's `opensUsed`:
/// the count of distinct devices that opened it, the one a `maxOpens`
/// limit counts. The secret and its hash are never in the answer.
fn link_row(mut r: Value) -> Value {
    let devices = r.get("opensUsed").cloned().unwrap_or(json!(0));
    r["devices"] = devices;
    r["locked"] = json!(r.get("lockedUntil").is_some_and(|v| !v.is_null()));
    r
}

#[derive(Deserialize)]
struct RevokeLinkBody {
    tenant: String,
    link: String,
}

/// Revoke a link (`std.space.revokeLink`): the door closes its grant
/// first, then takes back its rows and suspends its seat.
async fn revoke_link(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: RevokeLinkBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let id = id_of("link", &req.link)?;
    call.detail = format!("tenant={tenant} link={id}");
    let rows = who
        .door(c)?
        .op_present_in(&tenant, REVOKE_LINK, &json!({"link": id}))
        .await?;
    Ok(Reply::json(
        json!({ "revoked": rows.into_iter().next().unwrap_or(Value::Null) }),
    ))
}

#[derive(Deserialize)]
struct RevokeShareBody {
    tenant: String,
    seat: String,
}

/// Revoke a share (`std.space.revokeShare`): the guest's window closes at
/// their next call.
async fn revoke_share(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: RevokeShareBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let seat = id_of("seat", &req.seat)?;
    call.detail = format!("tenant={tenant} seat={seat}");
    let rows = who
        .door(c)?
        .op_present_in(&tenant, REVOKE_SHARE, &json!({"seat": seat}))
        .await?;
    Ok(Reply::json(
        json!({ "revoked": rows.into_iter().next().unwrap_or(Value::Null) }),
    ))
}

/// One page of a security log: the deployment's own record (G8), one
/// tenant's record, or the console's own audit log.
async fn security_page(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let limit = whole(query, "limit")?
        .unwrap_or(PAGE_DEFAULT)
        .clamp(1, PAGE_MAX);
    let source = query
        .get("source")
        .map(String::as_str)
        .unwrap_or("deployment");
    let text = |k: &str| {
        query
            .get(k)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    match source {
        "deployment" => {
            let mut params = json!({ "limit": limit });
            if let Some(k) = text("kind") {
                params["kind"] = json!(k);
            }
            if let Some(t) = text("tenant") {
                params["tenant"] = json!(id_of("tenant", &t)?);
            }
            for k in ["since", "until", "before"] {
                if let Some(n) = whole(query, k)? {
                    params[k] = json!(n);
                }
            }
            let rows = who.door(c)?.op(EVENTS, true, &params).await?;
            let next = (rows.len() as u64 == limit)
                .then(|| rows.last().and_then(|r| r.get("index")).cloned())
                .flatten();
            Ok(Reply::json(json!({
                "source": "deployment",
                "events": rows,
                "before": next,
            })))
        }
        "tenant" => {
            let tenant = id_of("tenant", &text("tenant").unwrap_or_default())?;
            let from = whole(query, "from")?.unwrap_or(0);
            let page = who.door(c)?.tenant_events(&tenant, from, limit).await?;
            let events = page
                .get("events")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new()));
            let full = events.as_array().is_some_and(|e| e.len() as u64 == limit);
            Ok(Reply::json(json!({
                "source": "tenant",
                "tenant": tenant,
                "events": events,
                "verifiableFrom": page.get("verifiableFrom"),
                "from": full.then(|| page.get("next").cloned()).flatten(),
            })))
        }
        "console" => {
            let prefix = text("action");
            let rows = c
                .store()
                .audit_page(
                    &call.deployment,
                    whole(query, "before")?,
                    prefix.as_deref(),
                    limit,
                )
                .await?;
            let next = (rows.len() as u64 == limit)
                .then(|| rows.last().map(|r| r.id))
                .flatten();
            Ok(Reply::json(json!({
                "source": "console",
                "events": rows.iter().map(|r| json!({
                    "id": r.id,
                    "at": r.at,
                    "actor": r.actor,
                    "action": r.action,
                    "outcome": r.outcome,
                    "detail": r.detail,
                })).collect::<Vec<_>>(),
                "before": next,
            })))
        }
        other => Err(ConsoleError::BadRequest(format!(
            "`source` is deployment, tenant or console, not `{other}`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_row_names_its_devices_and_whether_it_is_locked() {
        let r = link_row(json!({"id": "lnk_a", "opensUsed": 3, "opens": 5,
                                "lockedUntil": null}));
        assert_eq!(r["devices"], 3);
        assert_eq!(r["locked"], false);
        let r = link_row(json!({"id": "lnk_b", "opensUsed": 0, "lockedUntil": 99}));
        assert_eq!(r["locked"], true);
    }

    #[test]
    fn an_id_is_what_the_door_takes() {
        assert_eq!(id_of("link", " lnk_a-1 ").unwrap(), "lnk_a-1");
        for bad in ["", "a b", "a/b", "é"] {
            assert!(id_of("link", bad).is_err(), "{bad}");
        }
    }
}
