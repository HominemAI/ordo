//! **The pin stream, as a node reads it** (`plan-versions.md` §1.4 and
//! §1.8, increment 3, asks 16 and 17 ruled 2026-11-04).
//!
//! Every tenant is served the version it is pinned to. A node holds every
//! pin in memory ([`crate::deploy::Deployment`]'s pin map) and keeps it
//! current three ways:
//!
//! - **At boot**, a whole read ([`PinStream::reload`]).
//! - **On the nudge.** Every pin change notifies
//!   [`crate::versions::PINS_CHANNEL`] in its own transaction; the node
//!   then reads `$pins` from the last row it read ([`PinStream::tail`]).
//! - **On the reconcile tick**, `pins.reconcile_interval`
//!   ([`crate::versions::PINS_RECONCILE_ENV`], 30 seconds): a whole read
//!   again, so a missed nudge or a dropped listener costs at most one tick.
//!   A second region tails `$pins` the same way and needs no shared
//!   `NOTIFY`.
//!
//! Each read also takes the version settings (`crate::versions::
//! VersionSetting`, stored since 2026-11-05): a change to one nudges the
//! same channel, so `versions.new_tenant` reaches every node as a pin does.
//!
//! After each read the node loads every pinned version it does not serve
//! yet ([`PinStream::load_wanted`]), at most `versions.max_concurrent_loads`
//! at once ([`MAX_LOADS_ENV`], 1), and drops the ones no pin names once
//! they have been loaded for `versions.min_residency`. A version that
//! served nothing for `versions.idle_evict_after` is unloaded, and loaded
//! again here when a signed-in request for one of its tenants wakes it
//! (`Deployment::wake`). No anonymous call ever starts a load (V-29).
//!
//! A write a node makes under a stale pin is refused by the pin fence in
//! the tenant's own database (`ordo_exec::PIN_FENCE_DDL`), and the door
//! reads that tenant's pin again before it retries.

use std::sync::Arc;

use ordo_exec::{Pool, PoolConfig};

use crate::deploy::{self, Deployment, PinMap, Swapped};
use crate::versions::{VersionRefusal, Versions};

/// How many `$pins` rows one read of the stream takes.
const TAIL_BATCH: usize = 1_000;

/// `versions.max_concurrent_loads`: how many versions this node compiles
/// and gates at once. A node setting, so an environment variable.
pub const MAX_LOADS_ENV: &str = "ORDO_VERSIONS_MAX_CONCURRENT_LOADS";
/// Its default (§7).
pub const DEFAULT_MAX_LOADS: usize = 1;

/// `versions.max_loaded`: how many versions this node holds loaded at
/// once, the active one included. A load past it is refused, naming the
/// setting, and the version's tenants are closed on this node until a
/// version drops or the setting is raised.
pub const MAX_LOADED_ENV: &str = "ORDO_VERSIONS_MAX_LOADED";
/// Its default. A `Loaded` measured 0.05 to 3.6 MB in
/// `tests/versions_memory.rs`; its views are what cost, and the budget
/// below bounds those.
pub const DEFAULT_MAX_LOADED: usize = 8;

/// `versions.memory_budget`: the memory this node gives per-space views,
/// across every loaded version. Each version keeps at most its share of
/// [`VIEW_BYTES`]-sized views, the least recently used evicted first.
pub const MEMORY_BUDGET_ENV: &str = "ORDO_VERSIONS_MEMORY_BUDGET";
/// Its default: 512 MB.
pub const DEFAULT_MEMORY_BUDGET: u64 = 512 * 1024 * 1024;
/// What one per-space view is counted as: the largest measured, 222 KB for
/// teamspace, rounded up (`tests/versions_memory.rs`, 2026-11-23).
pub const VIEW_BYTES: u64 = 256 * 1024;

/// A node's limits on versions and views, read from the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeLimits {
    pub max_loads: usize,
    pub max_loaded: usize,
    pub memory_budget: u64,
}

impl Default for NodeLimits {
    fn default() -> NodeLimits {
        NodeLimits {
            max_loads: DEFAULT_MAX_LOADS,
            max_loaded: DEFAULT_MAX_LOADED,
            memory_budget: DEFAULT_MEMORY_BUDGET,
        }
    }
}

impl NodeLimits {
    /// From the three variables. Each one that does not parse is said,
    /// naming it, and its default is kept.
    pub fn from_env() -> (NodeLimits, Vec<String>) {
        NodeLimits::from(|v| std::env::var(v).ok())
    }

    /// [`NodeLimits::from_env`], reading through `read`.
    pub fn from(read: impl Fn(&str) -> Option<String>) -> (NodeLimits, Vec<String>) {
        let mut out = NodeLimits::default();
        let mut said = Vec::new();
        match parse_count(read(MAX_LOADS_ENV), MAX_LOADS_ENV, 16) {
            Ok(Some(n)) => out.max_loads = n,
            Ok(None) => {}
            Err(e) => said.push(e),
        }
        match parse_count(read(MAX_LOADED_ENV), MAX_LOADED_ENV, 256) {
            Ok(Some(n)) => out.max_loaded = n.max(1),
            Ok(None) => {}
            Err(e) => said.push(e),
        }
        match read(MEMORY_BUDGET_ENV).filter(|v| !v.trim().is_empty()) {
            None => {}
            Some(v) => match parse_bytes(&v) {
                Some(b) if b >= VIEW_BYTES => out.memory_budget = b,
                _ => said.push(format!(
                    "{MEMORY_BUDGET_ENV}={v} is not a size like `512MB` or `2GB` of at least \
                     256KB; the default, 512MB, is kept"
                )),
            },
        }
        (out, said)
    }

    /// The views one version may keep while `loaded` versions are loaded.
    pub fn views_per_version(&self, loaded: usize) -> u64 {
        (self.memory_budget / VIEW_BYTES / loaded.max(1) as u64).max(1)
    }
}

fn parse_count(v: Option<String>, name: &str, max: usize) -> Result<Option<usize>, String> {
    let Some(v) = v.filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    match v.trim().parse::<usize>() {
        Ok(n) if (1..=max).contains(&n) => Ok(Some(n)),
        _ => Err(format!(
            "{name}={v} is not a whole number from 1 to {max}; its default is kept"
        )),
    }
}

/// `512MB`, `2GB`, `256KB` or a number of bytes.
pub fn parse_bytes(v: &str) -> Option<u64> {
    let t = v.trim().to_ascii_uppercase();
    let (n, unit) = match t.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => (&t[..i], t[i..].trim()),
        None => (t.as_str(), ""),
    };
    let n: u64 = n.parse().ok()?;
    let mul = match unit {
        "" | "B" => 1,
        "KB" | "K" => 1024,
        "MB" | "M" => 1024 * 1024,
        "GB" | "G" => 1024 * 1024 * 1024,
        _ => return None,
    };
    n.checked_mul(mul)
}

/// What one read of the stream did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Read {
    /// Tenants whose pin this node changed.
    pub moved: Vec<String>,
    /// Versions loaded, and versions dropped.
    pub loaded: Vec<String>,
    pub dropped: Vec<String>,
    /// Versions a pin names that could not be loaded, with why.
    pub refused: Vec<(String, String)>,
    /// Versions unloaded for idleness (`versions.idle_evict_after`).
    pub evicted: Vec<String>,
    /// `versions.new_tenant`, when this read found it changed: the version
    /// it names now, `None` inside when it was cleared.
    pub new_tenant: Option<Option<String>>,
}

impl Read {
    /// Did the set of served versions change? The door's access engine
    /// and the routines are rebuilt when it did.
    pub fn versions_changed(&self) -> bool {
        !self.loaded.is_empty() || !self.dropped.is_empty() || !self.evicted.is_empty()
    }
}

/// One node's reader of the pins.
pub struct PinStream {
    deployment: Arc<Deployment>,
    /// The control database: `$spaces`, `$pins` and the registry.
    control: Pool,
    /// What a load dials its verification pool from.
    cfg: PoolConfig,
    keys: Arc<dyn ordo_crypto::KeyProvider>,
    space: ordo_crypto::SpaceId,
    /// One pass of loads at a time on this node, so a version is never
    /// loaded twice at once.
    loading: tokio::sync::Mutex<()>,
    /// `versions.max_concurrent_loads`: loads in one pass at once.
    max_loads: usize,
    /// `versions.max_loaded` and `versions.memory_budget`.
    limits: NodeLimits,
    /// The versions being loaded now, for per-node health.
    status: Option<Arc<crate::node_status::NodeStatus>>,
}

impl PinStream {
    pub fn new(
        deployment: Arc<Deployment>,
        control: Pool,
        cfg: PoolConfig,
        keys: Arc<dyn ordo_crypto::KeyProvider>,
        space: ordo_crypto::SpaceId,
    ) -> Arc<PinStream> {
        Arc::new(PinStream {
            deployment,
            control,
            cfg,
            keys,
            space,
            loading: tokio::sync::Mutex::new(()),
            max_loads: DEFAULT_MAX_LOADS,
            limits: NodeLimits::default(),
            status: None,
        })
    }

    /// The same stream, loading `max_loads` versions at once and saying on
    /// `/health/deploy` which it is loading.
    pub fn with_loads(
        deployment: Arc<Deployment>,
        control: Pool,
        cfg: PoolConfig,
        keys: Arc<dyn ordo_crypto::KeyProvider>,
        space: ordo_crypto::SpaceId,
        limits: NodeLimits,
        status: Arc<crate::node_status::NodeStatus>,
    ) -> Arc<PinStream> {
        Arc::new(PinStream {
            deployment,
            control,
            cfg,
            keys,
            space,
            loading: tokio::sync::Mutex::new(()),
            max_loads: limits.max_loads.max(1),
            limits,
            status: Some(status),
        })
    }

    /// Read every pin again and take them whole, then load what they name.
    pub async fn reload(&self) -> Result<Read, VersionRefusal> {
        self.read_moves().await;
        let before = self.deployment.pins();
        let (pins, seq) = Versions::on(&self.control).all_pins().await?;
        let mut map = PinMap {
            seq,
            ..PinMap::default()
        };
        for (space, version, epoch) in pins {
            map.by_space.insert(space, (version, epoch));
        }
        // Merged under the pin lock: a change the stream took while this
        // read ran is never moved back, nor its read position.
        let read_moved = self.deployment.replace_pins(map, &before);
        let mut read = Read {
            moved: read_moved,
            ..Read::default()
        };
        self.settle(&mut read).await;
        Ok(read)
    }

    /// Read the stream from the last row this node read, and take each
    /// change. Then load what the pins name.
    pub async fn tail(&self) -> Result<Read, VersionRefusal> {
        self.read_moves().await;
        let mut read = Read::default();
        loop {
            let from = self.deployment.pins().seq;
            let changes = Versions::on(&self.control)
                .pins_since(from, TAIL_BATCH)
                .await?;
            let n = changes.len();
            for c in changes {
                if self.deployment.note_pin(&c.space, &c.to, c.epoch, c.seq) {
                    read.moved.push(c.space);
                }
            }
            if n < TAIL_BATCH {
                break;
            }
        }
        self.settle(&mut read).await;
        Ok(read)
    }

    /// **The moves with dual writes** (increment 11a), read before the
    /// pins they ride: a pin taken after them carries the overlays its
    /// version writes with. Keyed by version, so overlays taken early are
    /// never run under the wrong one.
    async fn read_moves(&self) {
        match crate::dual_writes::read_all(&self.control).await {
            Ok(m) => self.deployment.set_moves(m),
            Err(e) => eprintln!("pins: the moves with dual writes were not read ({e})"),
        }
        match crate::grant_kills::read_all(&self.control).await {
            Ok(k) => self.deployment.set_grant_kills(k),
            Err(e) => eprintln!("pins: the grants moves killed were not read ({e})"),
        }
    }

    /// Load what the pins name and drop what they no longer do.
    async fn settle(&self, read: &mut Read) {
        // `versions.new_tenant` lives in the control database (owner,
        // 2026-11-05), and a change to it nudges this stream. Read before
        // the loads, so the version it names is loaded and kept.
        match Versions::on(&self.control).settings().await {
            Ok(s) => {
                self.deployment.set_guards(deploy::LoadGuards {
                    min_residency: s.min_residency_secs(),
                    idle_evict: s.idle_evict_secs(),
                });
                if self.deployment.new_tenant_setting() != s.new_tenant {
                    self.deployment.set_new_tenant(s.new_tenant.clone());
                    read.new_tenant = Some(s.new_tenant);
                }
            }
            Err(e) => eprintln!("versions: the version settings were not read ({e})"),
        }
        // The registry's active version is the deploy channel's to load,
        // with its live check and the tenant catch-up; never this stream's.
        match Versions::on(&self.control).active_version().await {
            Ok(v) => self.deployment.follow(v),
            Err(e) if e.is_missing_table() => self.deployment.follow(None),
            Err(e) => eprintln!("pins: the active version was not read ({e})"),
        }
        // A tenant whose pin moved onto an evicted version: its version is
        // wanted again.
        for space in &read.moved {
            if let Some((v, _)) = self.deployment.pin_of(space) {
                self.deployment.wake(&v);
            }
        }
        let (loaded, refused) = self.load_wanted().await;
        read.loaded = loaded;
        read.refused = refused;
        read.dropped = self.deployment.retain_pinned();
        read.evicted = self.deployment.evict_idle(crate::runtime::now_secs());
        self.evict_views();
    }

    /// Every version a pin names that this node does not serve.
    pub fn wanted(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .deployment
            .pins()
            .by_space
            .values()
            .map(|(v, _)| v.clone())
            // The version `versions.new_tenant` names, so the next tenant
            // is provisioned with its tables.
            .chain(self.deployment.new_tenant())
            .filter(|v| self.deployment.loaded(v).is_none())
            .filter(|v| self.deployment.following().as_deref() != Some(v.as_str()))
            .collect();
        let evicted = self.deployment.evicted();
        out.retain(|v| !evicted.contains(v));
        out.sort();
        out.dedup();
        out
    }

    /// Load every version [`PinStream::wanted`] names, one at a time. The
    /// versions loaded, and the ones refused with why. A refused version
    /// keeps its tenants closed (`SpaceRefusal::Unserved`) and is tried
    /// again on the next read; the refusal is the deployment owner's alert.
    pub async fn load_wanted(&self) -> (Vec<String>, Vec<(String, String)>) {
        use futures::StreamExt;
        let _turn = self.loading.lock().await;
        let wanted = self.wanted();
        if wanted.is_empty() {
            return (Vec::new(), Vec::new());
        }
        // Per-node health: these are loading, not loaded, until they land.
        if let Some(status) = &self.status {
            status.loading(wanted.clone());
            status.write().await;
        }
        let outcomes: Vec<(String, Result<Option<String>, String>)> = futures::stream::iter(wanted)
            .map(|version| async move {
                let outcome = self.load_one(&version).await;
                if let Some(status) = &self.status {
                    status.loaded_one(&version);
                }
                (version, outcome)
            })
            .buffer_unordered(self.max_loads)
            .collect()
            .await;
        let mut loaded = Vec::new();
        let mut refused = Vec::new();
        for (version, outcome) in outcomes {
            match outcome {
                Ok(Some(to)) => loaded.push(to),
                Ok(None) => {}
                Err(why) => {
                    // A request waiting on it stops waiting.
                    self.deployment.woken_refused(&version);
                    refused.push((version, why))
                }
            }
        }
        loaded.sort();
        refused.sort();
        (loaded, refused)
    }

    /// **Evict per-space views** (§1.5): in each loaded version, views idle
    /// for `versions.idle_evict_after`, then the least recently used past
    /// its share of `versions.memory_budget`. The number evicted.
    pub fn evict_views(&self) -> usize {
        let mut all = self.deployment.others();
        all.push(self.deployment.current());
        let keep = self.limits.views_per_version(all.len());
        let idle = self.deployment.guards().idle_evict.unwrap_or(u64::MAX);
        all.iter().map(|l| l.evict_views(idle, keep)).sum()
    }

    /// Load one version and serve it: `Some(name)` when it was installed,
    /// `None` when it already was.
    async fn load_one(&self, version: &str) -> Result<Option<String>, String> {
        let held = self.deployment.served_versions().len();
        if held >= self.limits.max_loaded {
            return Err(format!(
                "this node holds {held} versions, versions.max_loaded. Raise \
                 {MAX_LOADED_ENV}, or move tenants off a version so it drops"
            ));
        }
        match deploy::load_pinned(
            &self.control,
            &self.cfg,
            version,
            self.keys.clone(),
            self.space.clone(),
        )
        .await
        {
            Ok(v) if !v.custody_is(&self.keys) => {
                Err("it does not seal through this deployment's key provider".into())
            }
            Ok(v) => match self.deployment.install(v) {
                Swapped::Installed { to, .. } => Ok(Some(to)),
                Swapped::AlreadyServing(_) => Ok(None),
                Swapped::Refused(why) => Err(why),
            },
            Err(e) => Err(e.to_string()),
        }
    }
}

/// One line per thing a read did, for the node's log.
pub fn log_lines(read: &Read) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(named) = &read.new_tenant {
        out.push(format!(
            "versions: new tenants now start on {}",
            named.as_deref().unwrap_or("the active version")
        ));
    }
    if !read.moved.is_empty() {
        out.push(format!("pins: {} tenant(s) moved", read.moved.len()));
    }
    for v in &read.loaded {
        out.push(format!(
            "versions: now serving {v} beside the active version"
        ));
    }
    for v in &read.dropped {
        out.push(format!(
            "versions: {v} no longer served here: no tenant is pinned to it"
        ));
    }
    for v in &read.evicted {
        out.push(format!(
            "versions: {v} unloaded here: it served nothing for versions.idle_evict_after; \
             the next signed-in request for one of its tenants loads it again"
        ));
    }
    for (v, why) in &read.refused {
        out.push(format!(
            "versions: {v} is pinned and could not be loaded, so its tenants are closed: {why}"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_node_limits_read_their_variables_and_name_a_bad_one() {
        let (l, said) = NodeLimits::from(|_| None);
        assert_eq!(l, NodeLimits::default());
        assert!(said.is_empty());
        let (l, said) = NodeLimits::from(|v| match v {
            MAX_LOADS_ENV => Some("2".into()),
            MAX_LOADED_ENV => Some("4".into()),
            MEMORY_BUDGET_ENV => Some("2GB".into()),
            _ => None,
        });
        assert_eq!((l.max_loads, l.max_loaded), (2, 4));
        assert_eq!(l.memory_budget, 2 * 1024 * 1024 * 1024);
        assert!(said.is_empty());
        let (l, said) = NodeLimits::from(|v| match v {
            MAX_LOADS_ENV => Some("99".into()),
            MAX_LOADED_ENV => Some("none".into()),
            MEMORY_BUDGET_ENV => Some("lots".into()),
            _ => None,
        });
        assert_eq!(l, NodeLimits::default());
        assert_eq!(said.len(), 3);
        for (line, name) in said
            .iter()
            .zip([MAX_LOADS_ENV, MAX_LOADED_ENV, MEMORY_BUDGET_ENV])
        {
            assert!(line.contains(name), "{line}");
        }
    }

    #[test]
    fn the_budget_is_shared_between_loaded_versions() {
        let l = NodeLimits::default();
        // 512 MB at 256 KB a view.
        assert_eq!(l.views_per_version(1), 2048);
        assert_eq!(l.views_per_version(4), 512);
        assert_eq!(parse_bytes("256kb"), Some(256 * 1024));
        assert_eq!(parse_bytes("1 GB"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_bytes("12"), Some(12));
        assert_eq!(parse_bytes("1TB"), None);
    }
}
