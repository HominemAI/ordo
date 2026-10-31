//! This node's row in `$nodes` ([`ordo_exec::nodes`]): what it serves and
//! whether the tenant catch-up for that version has finished.
//!
//! Written at boot, after every swap once the access engine is rebuilt,
//! after every catch-up sweep, and every `ORDO_NODE_BEAT_SECS` (default 5)
//! so a node that stops without saying so drops out. Removed at shutdown.
//! `ordo apply` reads the rows to wait until a new version is served, and
//! `GET /health/deploy` shows them.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_exec::nodes::{self, Node};

/// The setting for how often a node writes its row.
pub const BEAT_SETTING: &str = "ORDO_NODE_BEAT_SECS";
pub const DEFAULT_BEAT_SECS: u64 = 5;

pub struct NodeStatus {
    pool: ordo_exec::Pool,
    row: Mutex<Node>,
    /// One write at a time, so a slow heartbeat cannot land after a newer
    /// write and put an older state back.
    writing: tokio::sync::Mutex<()>,
}

impl NodeStatus {
    pub fn new(pool: ordo_exec::Pool, node: String, beat: Duration) -> Arc<NodeStatus> {
        Arc::new(NodeStatus {
            pool,
            row: Mutex::new(Node {
                node,
                beat_secs: beat.as_secs().max(1),
                ..Node::default()
            }),
            writing: tokio::sync::Mutex::new(()),
        })
    }

    pub fn node(&self) -> String {
        self.row.lock().expect("node row lock").node.clone()
    }

    pub fn beat(&self) -> Duration {
        Duration::from_secs(self.row.lock().expect("node row lock").beat_secs)
    }

    /// What this node last said, for `/health/deploy`.
    pub fn snapshot(&self) -> Node {
        self.row.lock().expect("node row lock").clone()
    }

    /// The door now serves `version`: the swap is installed and the access
    /// engine rebuilt for it.
    pub async fn serving(&self, version: &str) {
        let v = if version == "(none)" { "" } else { version };
        self.row.lock().expect("node row lock").serving = v.to_string();
        self.write().await;
    }

    /// A catch-up sweep for `version` finished.
    pub async fn swept(&self, version: &str, held: Vec<(String, String)>, retrying: bool, ms: u64) {
        {
            let mut row = self.row.lock().expect("node row lock");
            row.caught_up = version.to_string();
            row.held = held;
            row.retrying = retrying;
            row.sweep_ms = ms;
        }
        self.write().await;
    }

    /// Write the row. A failure is a log line: the row is a report, and a
    /// node must not stop serving over it.
    pub async fn write(&self) {
        let _one = self.writing.lock().await;
        let row = self.snapshot();
        let mut out = nodes::beat(&self.pool, &row).await;
        if out.is_err() && nodes::ensure_table(&self.pool).await.is_ok() {
            out = nodes::beat(&self.pool, &row).await;
        }
        if let Err(e) = out {
            eprintln!(
                "node {}: its row in `$nodes` was not written ({e})",
                row.node
            );
        }
    }

    /// Write the row every beat until the task is aborted.
    pub fn spawn_heartbeat(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(me.beat());
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                me.write().await;
            }
        })
    }

    /// Remove the row, at shutdown.
    pub async fn leave(&self) {
        let _one = self.writing.lock().await;
        let _ = nodes::leave(&self.pool, &self.node()).await;
    }
}
