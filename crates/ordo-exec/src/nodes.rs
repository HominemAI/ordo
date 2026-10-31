//! **What each running node serves**, one row per node in the control
//! database, so `ordo apply` can wait until a new version is really served.
//!
//! An apply stores and activates a version and nudges the nodes on
//! `ordo.deploy`. Each node then swaps, rebuilds its access engine and
//! sweeps the tenants' databases forward (`tenant_forward`). Until all of
//! that is done, a page that calls an op the new version added is refused.
//! The row is how a node says it is done, and the apply reads it.
//!
//! # The row
//!
//! - `serving`: the version this node's door serves, written after the
//!   access engine is rebuilt for it. Empty while no version is active.
//! - `caught_up`: the version whose tenant catch-up last finished here.
//! - `held`: the databases that sweep did not bring forward, with reasons,
//!   as a JSON array of `[label, reason]` pairs.
//! - `retrying`: some of `held` may pass on the next sweep (unreachable,
//!   busy). A refused change does not set it.
//! - `beat_secs` and `seen_at`: the node writes the row every `beat_secs`.
//!   A row older than [`stale_after`] is a node that stopped without saying
//!   so, and nothing waits for it.
//!
//! `seen_at` is the database's clock, both when written and when read, so
//! two machines with different clocks still agree on which rows are live.
//!
//! Like `$fleet` this is a report and never an authority. Nothing routes or
//! refuses on it. The serving version is the registry's; a tenant's
//! version is in its own `$migrations`.

use crate::{sql_lit, Pool, PoolError};

pub const NODES_TABLE: &str = "$nodes";

pub const NODES_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$nodes\" ( \
  \"node\" text NOT NULL PRIMARY KEY, \
  \"serving\" text NOT NULL, \
  \"caught_up\" text NOT NULL, \
  \"held\" text NOT NULL, \
  \"retrying\" boolean NOT NULL, \
  \"sweep_ms\" bigint NOT NULL, \
  \"beat_secs\" bigint NOT NULL, \
  \"seen_at\" bigint NOT NULL \
)";

/// What one node last said about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Node {
    pub node: String,
    pub serving: String,
    pub caught_up: String,
    pub held: Vec<(String, String)>,
    pub retrying: bool,
    /// How long the last finished catch-up sweep took.
    pub sweep_ms: u64,
    pub beat_secs: u64,
    /// Seconds since this node last wrote its row, by the database's clock.
    pub age_secs: u64,
}

/// How old a row may be before its node counts as gone: three missed beats
/// and two seconds of slack.
pub fn stale_after(beat_secs: u64) -> u64 {
    beat_secs.max(1) * 3 + 2
}

pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    // Retried once: `CREATE TABLE IF NOT EXISTS` is not race-free between
    // two booting nodes.
    if pool.rows(NODES_DDL).await.is_err() {
        pool.rows(NODES_DDL).await?;
    }
    Ok(())
}

/// Write this node's row, stamped with the database's clock.
pub async fn beat(pool: &Pool, n: &Node) -> Result<(), PoolError> {
    let held = serde_json::to_string(&n.held).unwrap_or_else(|_| "[]".into());
    pool.rows(&format!(
        "INSERT INTO \"{NODES_TABLE}\" (\"node\", \"serving\", \"caught_up\", \"held\", \
           \"retrying\", \"sweep_ms\", \"beat_secs\", \"seen_at\") \
         VALUES ({}, {}, {}, {}, {}, {}, {}, \
           extract(epoch from clock_timestamp())::bigint) \
         ON CONFLICT (\"node\") DO UPDATE SET \
           \"serving\" = EXCLUDED.\"serving\", \"caught_up\" = EXCLUDED.\"caught_up\", \
           \"held\" = EXCLUDED.\"held\", \"retrying\" = EXCLUDED.\"retrying\", \
           \"sweep_ms\" = EXCLUDED.\"sweep_ms\", \"beat_secs\" = EXCLUDED.\"beat_secs\", \
           \"seen_at\" = EXCLUDED.\"seen_at\"",
        sql_lit(&n.node),
        sql_lit(&n.serving),
        sql_lit(&n.caught_up),
        sql_lit(&held),
        n.retrying,
        n.sweep_ms,
        n.beat_secs.max(1),
    ))
    .await?;
    Ok(())
}

/// Remove this node's row: called when it shuts down, so an apply does not
/// wait for a node that is gone.
pub async fn leave(pool: &Pool, node: &str) -> Result<(), PoolError> {
    pool.rows(&format!(
        "DELETE FROM \"{NODES_TABLE}\" WHERE \"node\" = {}",
        sql_lit(node)
    ))
    .await?;
    Ok(())
}

/// Every node whose row is fresh, in node order. An absent table is no
/// nodes: a deployment whose nodes predate this table has none to wait for.
pub async fn live(pool: &Pool) -> Result<Vec<Node>, PoolError> {
    let rows = match pool
        .rows(&format!(
            "SELECT \"node\", \"serving\", \"caught_up\", \"held\", \"retrying\"::text, \
               \"sweep_ms\", \"beat_secs\", \
               extract(epoch from clock_timestamp())::bigint - \"seen_at\" \
             FROM \"{NODES_TABLE}\" ORDER BY \"node\""
        ))
        .await
    {
        Ok(r) => r,
        // Told by SQLSTATE: a table or a column a newer node added.
        Err(e) if e.is_missing_table() || e.is_missing_column() => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(rows
        .iter()
        .map(|row| {
            let cell = |i: usize| row.get(i).and_then(|c| c.as_deref()).unwrap_or_default();
            Node {
                node: cell(0).to_string(),
                serving: cell(1).to_string(),
                caught_up: cell(2).to_string(),
                held: serde_json::from_str(cell(3)).unwrap_or_default(),
                retrying: matches!(cell(4), "true" | "t"),
                sweep_ms: cell(5).parse().unwrap_or(0),
                beat_secs: cell(6).parse().unwrap_or(1),
                age_secs: cell(7).parse::<i64>().unwrap_or(0).max(0) as u64,
            }
        })
        .filter(|n| n.age_secs <= stale_after(n.beat_secs))
        .collect())
}

/// Where a version stands across the live nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// Live nodes that serve the version and finished its catch-up with
    /// nothing left to retry.
    pub done: Vec<String>,
    /// `(node, what it is still doing)` for each live node that is not.
    pub pending: Vec<(String, String)>,
    /// `(node, database, reason)` for each database a finished sweep left
    /// behind. Its tenant stays closed until the reason is fixed.
    pub held: Vec<(String, String, String)>,
}

impl Progress {
    /// True when no live node is still working on the version. A held
    /// database does not stop it: the version is served, and that tenant
    /// is closed with its reason.
    pub fn settled(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Where `version` stands on `nodes`.
pub fn progress(nodes: &[Node], version: &str) -> Progress {
    let mut out = Progress::default();
    for n in nodes {
        let pending = if n.serving != version {
            let now = if n.serving.is_empty() {
                "nothing".to_string()
            } else {
                n.serving.clone()
            };
            Some(format!("still serves {now}"))
        } else if n.caught_up != version {
            Some("serves it; the tenant catch-up is running".to_string())
        } else if n.retrying {
            Some(format!(
                "serves it; {} tenant database(s) will be tried again",
                n.held.len()
            ))
        } else {
            None
        };
        if n.serving == version && n.caught_up == version {
            for (db, why) in &n.held {
                out.held.push((n.node.clone(), db.clone(), why.clone()));
            }
        }
        match pending {
            Some(what) => out.pending.push((n.node.clone(), what)),
            None => out.done.push(n.node.clone()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, serving: &str, caught_up: &str) -> Node {
        Node {
            node: name.into(),
            serving: serving.into(),
            caught_up: caught_up.into(),
            beat_secs: 5,
            ..Node::default()
        }
    }

    #[test]
    fn a_version_is_settled_only_when_every_node_serves_it_and_caught_up() {
        let nodes = vec![node("a", "v2", "v2"), node("b", "v2", "v1")];
        let p = progress(&nodes, "v2");
        assert!(!p.settled());
        assert_eq!(p.done, vec!["a".to_string()]);
        assert_eq!(p.pending[0].0, "b");

        let nodes = vec![node("a", "v2", "v2"), node("b", "v1", "v1")];
        let p = progress(&nodes, "v2");
        assert_eq!(p.pending, vec![("b".into(), "still serves v1".into())]);

        let p = progress(&[node("a", "v2", "v2")], "v2");
        assert!(p.settled());
        assert!(progress(&[], "v2").settled());
    }

    #[test]
    fn a_hold_that_may_pass_keeps_waiting_and_a_refusal_is_reported() {
        let mut n = node("a", "v2", "v2");
        n.held = vec![("db1".into(), "unreachable: refused".into())];
        n.retrying = true;
        let p = progress(std::slice::from_ref(&n), "v2");
        assert!(!p.settled());
        assert_eq!(p.held.len(), 1);

        n.retrying = false;
        let p = progress(&[n], "v2");
        assert!(p.settled());
        assert_eq!(
            p.held,
            vec![("a".into(), "db1".into(), "unreachable: refused".into())]
        );
    }

    #[test]
    fn a_node_is_gone_after_three_missed_beats() {
        assert_eq!(stale_after(5), 17);
        assert_eq!(stale_after(0), 5);
    }
}
