//! **Sinks for a staging clone** (`plan-versions.md` §3.4, ruled 2026-11-02,
//! D17: "In staging, mail, push and egress go to sinks").
//!
//! Nothing a clone does reaches a real person or an outside service. What it
//! would have sent is written to `$sink`, in the deployment's control
//! database, and read back with `std.deployment.sinks` (`ordo clones sink`).
//!
//! # What goes to the sink
//!
//! - **Mail.** A routine's `notify` in a clone tells nobody: each holder it
//!   would have mailed is one entry, with the notice's text
//!   ([`crate::routines`]). A cloned member has no address in staging
//!   anyway; the sink is what makes that a rule and not a coincidence.
//! - **Egress.** A function's `fetch` in a clone passes every check the
//!   real kernel makes before the network ([`ordo_egress::FnEgress::check_only`]),
//!   so a call production would refuse is refused here too. Then nothing is
//!   dialled and no secret is opened: the call is recorded, and the function
//!   is answered `200` with the body `{}` and the header
//!   [`SINK_HEADER`]`: recorded`.
//! - **Push.** Ordo sends no push today. [`SinkKind::Push`] is the place a
//!   push channel records to when one is built; it must ask [`is_clone`]
//!   first, as mail and egress do.
//!
//! # Which tenants
//!
//! Every tenant in `$clones`, always. It is read from the control database
//! at the moment of sending, not from a node's memory, so a node that has
//! not seen a clone yet still sinks it. A read that fails refuses the send.
//! The person who opens a clone may run every op, writes included: what
//! those writes would send lands here.

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use ordo_egress::{
    FetchError, FnEgress, FnFetch, FnFetchRequest, FnFetchResponse, FnMacAnswer, FnMacRequest,
};
use ordo_exec::{sql_lit, Pool, PoolError};
use serde_json::{json, Value};

/// The header a sunk call's answer carries.
pub const SINK_HEADER: &str = "x-ordo-sink";

/// The most of a body an entry keeps, in bytes. The rest is counted.
pub const BODY_KEPT: usize = 16 * 1024;

/// The most entries one read answers.
pub const READ_MAX: u32 = 500;

const SINK_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$sink\" ( \
  \"id\" bigserial PRIMARY KEY, \
  \"space\" text NOT NULL, \
  \"kind\" text NOT NULL, \
  \"target\" text NOT NULL, \
  \"summary\" text NOT NULL, \
  \"body\" text NOT NULL, \
  \"bytes\" bigint NOT NULL, \
  \"at\" bigint NOT NULL \
); \
CREATE INDEX IF NOT EXISTS \"$sink_space\" ON \"$sink\" (\"space\", \"id\")";

/// What a sink entry stands in for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkKind {
    Mail,
    Push,
    Egress,
}

impl SinkKind {
    pub fn word(self) -> &'static str {
        match self {
            SinkKind::Mail => "mail",
            SinkKind::Push => "push",
            SinkKind::Egress => "egress",
        }
    }
}

/// One thing a clone would have sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkEntry {
    pub id: i64,
    pub space: String,
    pub kind: String,
    /// Who or where: a member's principal, or `METHOD url`.
    pub target: String,
    /// A line: the notice's text, or the headers' names.
    pub summary: String,
    /// The body, up to [`BODY_KEPT`] bytes.
    pub body: String,
    /// The whole body's size.
    pub bytes: u64,
    pub at: u64,
}

impl SinkEntry {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id, "space": self.space, "kind": self.kind,
            "target": self.target, "summary": self.summary, "body": self.body,
            "bytes": self.bytes, "at": self.at,
        })
    }
}

/// **Whether `space` is a staging clone here**, read from `$clones`. A
/// deployment that never cloned has no table, and nothing is a clone.
pub async fn is_clone(pool: &Pool, space: &str) -> Result<bool, PoolError> {
    let sql = format!(
        "SELECT 1 FROM \"$clones\" WHERE \"space\" = {}",
        sql_lit(space)
    );
    match pool.rows(&sql).await {
        Ok(rows) => Ok(!rows.is_empty()),
        Err(e) if e.is_missing_table() => Ok(false),
        Err(e) => Err(e),
    }
}

/// **Record one thing a clone would have sent.**
pub async fn record(
    pool: &Pool,
    space: &str,
    kind: SinkKind,
    target: &str,
    summary: &str,
    body: &[u8],
    at: u64,
) -> Result<(), PoolError> {
    pool.rows(SINK_DDL).await?;
    let kept = String::from_utf8_lossy(&body[..body.len().min(BODY_KEPT)]).into_owned();
    pool.rows(&format!(
        "INSERT INTO \"$sink\" (\"space\", \"kind\", \"target\", \"summary\", \"body\", \
         \"bytes\", \"at\") VALUES ({}, {}, {}, {}, {}, {}, {at})",
        sql_lit(space),
        sql_lit(kind.word()),
        sql_lit(target),
        sql_lit(summary),
        sql_lit(&kept.replace('\0', "")),
        body.len()
    ))
    .await
    .map(|_| ())
}

/// The newest entries first, of one space or of all, at most `limit`
/// ([`READ_MAX`] at most).
pub async fn entries(
    pool: &Pool,
    space: Option<&str>,
    limit: u32,
) -> Result<Vec<SinkEntry>, PoolError> {
    let filter = space
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("WHERE \"space\" = {}", sql_lit(s.trim())))
        .unwrap_or_default();
    let sql = format!(
        "SELECT \"id\"::text, \"space\", \"kind\", \"target\", \"summary\", \"body\", \
         \"bytes\"::text, \"at\"::text FROM \"$sink\" {filter} ORDER BY \"id\" DESC LIMIT {}",
        limit.clamp(1, READ_MAX)
    );
    let rows = match pool.rows(&sql).await {
        Ok(rows) => rows,
        Err(e) if e.is_missing_table() => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let g = |i: usize| r.get(i).cloned().flatten();
            Some(SinkEntry {
                id: g(0)?.parse().ok()?,
                space: g(1)?,
                kind: g(2)?,
                target: g(3)?,
                summary: g(4)?,
                body: g(5)?,
                bytes: g(6)?.parse().ok()?,
                at: g(7)?.parse().ok()?,
            })
        })
        .collect())
}

// ── egress ───────────────────────────────────────────────────────────────

/// **A function's `fetch`, with a clone's calls sunk.** Every other space's
/// calls go out through the egress kernel as before.
pub struct SinkFetch {
    egress: Arc<FnEgress>,
    pool: Pool,
    /// Spaces read as clones. A clone stays one, so a yes is kept; a no is
    /// read again on the next call.
    clones: RwLock<BTreeSet<String>>,
}

impl SinkFetch {
    pub fn new(egress: FnEgress, pool: Pool) -> SinkFetch {
        SinkFetch {
            egress: Arc::new(egress),
            pool,
            clones: RwLock::new(BTreeSet::new()),
        }
    }

    async fn sunk(&self, space: &str) -> Result<bool, FetchError> {
        if self
            .clones
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .contains(space)
        {
            return Ok(true);
        }
        let yes = is_clone(&self.pool, space)
            .await
            .map_err(|e| FetchError::Store {
                detail: format!("whether this tenant is a staging clone could not be read: {e}"),
            })?;
        if yes {
            self.clones
                .write()
                .unwrap_or_else(|p| p.into_inner())
                .insert(space.to_string());
        }
        Ok(yes)
    }
}

#[async_trait]
impl FnFetch for SinkFetch {
    async fn fetch(&self, space: &str, req: FnFetchRequest) -> Result<FnFetchResponse, FetchError> {
        if !self.sunk(space).await? {
            return self.egress.fetch(space, req).await;
        }
        let (url, method) = self.egress.check_only(&req)?;
        let names: Vec<String> = req
            .headers
            .iter()
            .map(|(n, _)| n.to_ascii_lowercase())
            .collect();
        let summary = if names.is_empty() {
            "no headers".to_string()
        } else {
            format!("headers: {}", names.join(", "))
        };
        record(
            &self.pool,
            space,
            SinkKind::Egress,
            &format!("{method} {url}"),
            &summary,
            &req.body,
            crate::runtime::now_secs(),
        )
        .await
        .map_err(|e| FetchError::Store {
            detail: format!("the staging sink could not record the call: {e}"),
        })?;
        Ok(FnFetchResponse {
            status: 200,
            headers: vec![
                (SINK_HEADER.to_string(), "recorded".to_string()),
                ("content-type".to_string(), "application/json".to_string()),
            ],
            body: b"{}".to_vec(),
        })
    }

    /// An HMAC reaches nothing outside: it is computed as anywhere else.
    async fn mac(&self, space: &str, req: FnMacRequest) -> Result<FnMacAnswer, FetchError> {
        self.egress.mac(space, req).await
    }
}
