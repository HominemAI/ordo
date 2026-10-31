//! `ordo apply` waits until the version it activated is served.
//!
//! An activation stores the version and nudges the nodes. Each node then
//! swaps, rebuilds its access engine and brings the tenants' databases
//! forward. Until that is done, a page that calls an op the new version
//! added is refused. So the apply waits by default, on what each node says
//! in `$nodes` ([`ordo_exec::nodes`]): read from the database on the direct
//! path, and from `GET /health/deploy` with `--host`.
//!
//! - `--no-wait` returns once the version is active, as before 2026-10-22.
//! - `--wait-secs <n>`, or `ORDO_APPLY_WAIT_SECS`, bounds the wait
//!   (default 300). Past it the apply exits 1 and names what each node is
//!   still doing. The version stays active.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::operator::Flags;

/// The setting that bounds the wait.
pub const WAIT_SETTING: &str = "ORDO_APPLY_WAIT_SECS";
pub const DEFAULT_WAIT_SECS: u64 = 300;
/// How often the nodes are asked again.
const POLL: Duration = Duration::from_millis(250);

/// How long to wait, or `None` for `--no-wait`.
pub fn from_flags(flags: &Flags) -> Result<Option<Duration>, String> {
    if flags.has("--no-wait") {
        if flags.named_value("--wait-secs").is_some() {
            return Err("`--no-wait` and `--wait-secs` say opposite things; give one".into());
        }
        return Ok(None);
    }
    let (raw, from) = match flags.named_value("--wait-secs") {
        Some(v) => (Some(v.to_string()), "--wait-secs"),
        None => (
            std::env::var(WAIT_SETTING)
                .ok()
                .filter(|v| !v.trim().is_empty()),
            WAIT_SETTING,
        ),
    };
    match raw {
        None => Ok(Some(Duration::from_secs(DEFAULT_WAIT_SECS))),
        Some(v) => v
            .trim()
            .parse::<u64>()
            .map(|s| Some(Duration::from_secs(s)))
            .map_err(|_| format!("{from} `{v}` is not a number of seconds")),
    }
}

/// What the nodes said on one look.
#[derive(Debug, Default)]
pub struct Seen {
    pub live: usize,
    /// `(node, what it is still doing)`.
    pub pending: Vec<(String, String)>,
    /// One line for each database a finished sweep left behind.
    pub held: Vec<String>,
    /// The longest catch-up sweep among the nodes.
    pub sweep_ms: u64,
}

/// One look at `$nodes` in the database.
pub async fn look_in_database(pool: &ordo_exec::Pool, version: &str) -> Result<Seen, String> {
    let nodes = ordo_exec::nodes::live(pool)
        .await
        .map_err(|e| format!("the nodes could not be read: {e}"))?;
    let p = ordo_exec::nodes::progress(&nodes, version);
    Ok(Seen {
        live: nodes.len(),
        pending: p.pending,
        held: p
            .held
            .into_iter()
            .map(|(node, db, why)| format!("{db} (on {node}): {why}"))
            .collect(),
        sweep_ms: nodes.iter().map(|n| n.sweep_ms).max().unwrap_or(0),
    })
}

/// Read a `GET /health/deploy` body.
///
/// An anonymous asker gets `{"status", "settled"}` and no node detail
/// (audit F12); `settled: false` then means something is still pending,
/// though not which node.
pub fn seen_from_status(body: &Value) -> Seen {
    let nodes = body["nodes"].as_array().cloned().unwrap_or_default();
    let held = body["held"].as_u64().unwrap_or(0);
    let mut pending: Vec<(String, String)> = body["pending"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            (
                p["node"].as_str().unwrap_or("?").to_string(),
                p["doing"].as_str().unwrap_or("?").to_string(),
            )
        })
        .collect();
    if pending.is_empty() && body["settled"] == Value::Bool(false) {
        pending.push((
            "a node".to_string(),
            "not settled yet (the detail is the deployment owner's)".to_string(),
        ));
    }
    Seen {
        live: nodes.len(),
        pending,
        held: (held > 0)
            .then(|| {
                format!("{held} tenant database(s); `ordo apply` against the database names them")
            })
            .into_iter()
            .collect(),
        sweep_ms: nodes
            .iter()
            .filter_map(|n| n["sweepMs"].as_u64())
            .max()
            .unwrap_or(0),
    }
}

/// Look until every live node serves `version` and has caught its tenants
/// up, or `limit` passes. `Ok` is the lines to print; `Err` is the timeout
/// report.
pub async fn until_served<F, Fut>(
    version: &str,
    limit: Duration,
    mut look: F,
) -> Result<String, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Seen, String>>,
{
    let started = Instant::now();
    let mut last: Result<Seen, String>;
    loop {
        last = look().await;
        if let Ok(seen) = &last {
            if seen.pending.is_empty() {
                return Ok(served(version, seen, started.elapsed()));
            }
        }
        if started.elapsed() >= limit {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    let mut out = format!(
        "served     not yet: waited {}s. {version} is active, and these nodes are not done:\n",
        limit.as_secs()
    );
    match last {
        Ok(seen) => {
            for (node, doing) in &seen.pending {
                out.push_str(&format!("           {node}: {doing}\n"));
            }
            for line in &seen.held {
                out.push_str(&format!("held       {line}\n"));
            }
        }
        Err(e) => out.push_str(&format!("           {e}\n")),
    }
    out.push_str(&format!(
        "           wait longer with --wait-secs <n> or {WAIT_SETTING}, or return at once \
         with --no-wait\n"
    ));
    Err(out)
}

fn served(version: &str, seen: &Seen, took: Duration) -> String {
    let mut out = if seen.live == 0 {
        format!("served     no node is running; the first one to start serves {version}\n")
    } else {
        format!(
            "served     {version} on {} node(s), tenants caught up (waited {:.1}s; slowest \
             catch-up {:.1}s)\n",
            seen.live,
            took.as_secs_f64(),
            seen.sweep_ms as f64 / 1000.0
        )
    };
    for line in &seen.held {
        out.push_str(&format!("held       {line}\n"));
    }
    if !seen.held.is_empty() {
        out.push_str(
            "           a held tenant stays closed until its database is brought forward; \
             `/health/ready` gives the reason\n",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn an_anonymous_status_still_says_whether_it_has_settled() {
        let pending = seen_from_status(&serde_json::json!({"status": "pending", "settled": false}));
        assert_eq!(
            pending.pending.len(),
            1,
            "unsettled must keep the wait going"
        );
        let done = seen_from_status(&serde_json::json!({"status": "settled", "settled": true}));
        assert!(done.pending.is_empty());
    }

    #[tokio::test]
    async fn the_wait_returns_when_every_node_is_done() {
        let looks = AtomicUsize::new(0);
        let out = until_served("v2", Duration::from_secs(5), || {
            let n = looks.fetch_add(1, Ordering::SeqCst);
            async move {
                Ok(Seen {
                    live: 1,
                    pending: if n < 2 {
                        vec![("a".into(), "still serves v1".into())]
                    } else {
                        vec![]
                    },
                    ..Seen::default()
                })
            }
        })
        .await
        .expect("served");
        assert!(out.contains("served     v2 on 1 node(s)"), "{out}");
        assert_eq!(looks.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn the_timeout_names_what_is_pending() {
        let out = until_served("v2", Duration::from_millis(300), || async {
            Ok(Seen {
                live: 1,
                pending: vec![(
                    "a".into(),
                    "serves it; the tenant catch-up is running".into(),
                )],
                ..Seen::default()
            })
        })
        .await
        .expect_err("times out");
        assert!(
            out.contains("a: serves it; the tenant catch-up is running"),
            "{out}"
        );
        assert!(out.contains("--wait-secs"), "{out}");
    }

    #[test]
    fn the_status_body_reads_back() {
        let body = serde_json::json!({
            "version": "v2", "settled": false, "held": 2,
            "pending": [{ "node": "a", "doing": "still serves v1" }],
            "nodes": [{ "node": "a", "sweepMs": 1200 }],
        });
        let seen = seen_from_status(&body);
        assert_eq!(seen.live, 1);
        assert_eq!(
            seen.pending,
            vec![("a".to_string(), "still serves v1".to_string())]
        );
        assert_eq!(seen.held.len(), 1);
        assert_eq!(seen.sweep_ms, 1200);
    }
}
