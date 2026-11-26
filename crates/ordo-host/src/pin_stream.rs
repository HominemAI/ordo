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

/// What one per-space view is counted as against `versions.memory_budget`:
/// the largest measured, 222 KB for teamspace, rounded up
/// (`tests/versions_memory.rs`, 2026-11-23). A measurement, not a policy:
/// the budget is the setting.
pub const VIEW_BYTES: u64 = 256 * 1024;

/// The views one version may keep while `loaded` versions are loaded under
/// a budget of `budget` bytes.
pub fn views_per_version(budget: u64, loaded: usize) -> u64 {
    (budget / VIEW_BYTES / loaded.max(1) as u64).max(1)
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
    /// Versions pushed out to make room for another under
    /// `versions.max_loaded` (`versions.at_capacity` `evict-idle`): each
    /// with the version it made room for.
    pub pushed_out: Vec<(String, String)>,
    /// `versions.new_tenant`, when this read found it changed: the version
    /// it names now, `None` inside when it was cleared.
    pub new_tenant: Option<Option<String>>,
}

impl Read {
    /// Did the set of served versions change? The door's access engine
    /// and the routines are rebuilt when it did.
    pub fn versions_changed(&self) -> bool {
        !self.loaded.is_empty()
            || !self.dropped.is_empty()
            || !self.evicted.is_empty()
            || !self.pushed_out.is_empty()
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
    /// loaded twice at once. `versions.max_concurrent_loads`,
    /// `versions.max_loaded`, `versions.memory_budget` and
    /// `versions.at_capacity` are read from the stored settings on each
    /// read ([`Deployment::guards`]).
    loading: tokio::sync::Mutex<()>,
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
            status: None,
        })
    }

    /// The same stream, saying on `/health/deploy` which versions it is
    /// loading.
    pub fn with_status(
        deployment: Arc<Deployment>,
        control: Pool,
        cfg: PoolConfig,
        keys: Arc<dyn ordo_crypto::KeyProvider>,
        space: ordo_crypto::SpaceId,
        status: Arc<crate::node_status::NodeStatus>,
    ) -> Arc<PinStream> {
        Arc::new(PinStream {
            deployment,
            control,
            cfg,
            keys,
            space,
            loading: tokio::sync::Mutex::new(()),
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
                self.deployment.set_guards(deploy::LoadGuards::of(&s));
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
        let (loaded, refused, pushed) = self.load_wanted().await;
        read.loaded = loaded;
        read.refused = refused;
        read.pushed_out = pushed;
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
    #[allow(clippy::type_complexity)]
    pub async fn load_wanted(&self) -> (Vec<String>, Vec<(String, String)>, Vec<(String, String)>) {
        use futures::StreamExt;
        let _turn = self.loading.lock().await;
        let wanted = self.wanted();
        if wanted.is_empty() {
            return (Vec::new(), Vec::new(), Vec::new());
        }
        let max_loads = self.deployment.guards().max_loads.max(1);
        // Per-node health: these are loading, not loaded, until they land.
        if let Some(status) = &self.status {
            status.loading(wanted.clone());
            status.write().await;
        }
        #[allow(clippy::type_complexity)]
        let outcomes: Vec<(String, Result<(Option<String>, Option<String>), String>)> =
            futures::stream::iter(wanted)
                .map(|version| async move {
                    let outcome = self.load_one(&version).await;
                    if let Some(status) = &self.status {
                        status.loaded_one(&version);
                    }
                    (version, outcome)
                })
                .buffer_unordered(max_loads)
                .collect()
                .await;
        let mut loaded = Vec::new();
        let mut refused = Vec::new();
        let mut pushed = Vec::new();
        for (version, outcome) in outcomes {
            match outcome {
                Ok((installed, out)) => {
                    if let Some(out) = out {
                        pushed.push((out, version.clone()));
                    }
                    if let Some(to) = installed {
                        loaded.push(to);
                    }
                }
                Err(why) => {
                    // A request waiting on it stops waiting.
                    self.deployment.woken_refused(&version);
                    refused.push((version, why))
                }
            }
        }
        loaded.sort();
        refused.sort();
        pushed.sort();
        (loaded, refused, pushed)
    }

    /// **Evict per-space views** (§1.5): in each loaded version, views idle
    /// for `versions.idle_evict_after`, then the least recently used past
    /// its share of `versions.memory_budget`. The number evicted.
    pub fn evict_views(&self) -> usize {
        let mut all = self.deployment.others();
        all.push(self.deployment.current());
        let guards = self.deployment.guards();
        let keep = views_per_version(guards.memory_budget, all.len());
        let idle = guards.idle_evict.unwrap_or(u64::MAX);
        all.iter().map(|l| l.evict_views(idle, keep)).sum()
    }

    /// Load one version and serve it: `Some(name)` when it was installed,
    /// `None` when it already was; and the version pushed out to make room
    /// for it, if one was.
    ///
    /// **At `versions.max_loaded`** (owner, 2026-11-25): with
    /// `versions.at_capacity` `evict-idle`, the default, the least recently
    /// used idle version beside the active one is pushed out
    /// ([`Deployment::evict_for_room`]) and loads again when a signed-in
    /// request for one of its tenants wakes it. With none idle, or with
    /// `refuse`, the load is refused, naming both settings.
    async fn load_one(&self, version: &str) -> Result<(Option<String>, Option<String>), String> {
        let guards = self.deployment.guards();
        let held = self.deployment.served_versions().len();
        let mut pushed = None;
        if held >= guards.max_loaded {
            let room = match guards.at_capacity {
                crate::versions::AtCapacity::EvictIdle => self
                    .deployment
                    .evict_for_room(crate::runtime::now_secs(), version),
                crate::versions::AtCapacity::Refuse => None,
            };
            match room {
                Some(out) => pushed = Some(out),
                None => {
                    return Err(format!(
                        "this node holds {held} versions, versions.max_loaded{}. Raise it with \
                         `ordo versions set max-loaded <n>`, or move tenants off a version so \
                         it drops",
                        match guards.at_capacity {
                            crate::versions::AtCapacity::Refuse => {
                                ", and versions.at_capacity is `refuse`; \
                                 `ordo versions set at-capacity evict-idle` pushes out the \
                                 least used idle version instead"
                            }
                            crate::versions::AtCapacity::EvictIdle => {
                                ", and none beside the active one is idle: each loaded within \
                                 versions.min_residency or named by versions.new_tenant"
                            }
                        }
                    ));
                }
            }
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
                Swapped::Installed { to, .. } => Ok((Some(to), pushed)),
                Swapped::AlreadyServing(_) => Ok((None, pushed)),
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
    for (gone, room_for) in &read.pushed_out {
        out.push(format!(
            "versions: {gone} unloaded here to make room for {room_for} under \
             versions.max_loaded (versions.at_capacity is evict-idle); the next signed-in \
             request for one of its tenants loads it again"
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
    use crate::versions::{check_guard, parse_bytes, VersionSetting, DEFAULT_MEMORY_BUDGET};

    #[test]
    fn the_node_limits_are_checked_as_settings_and_a_bad_one_is_refused() {
        assert_eq!(
            check_guard(VersionSetting::MaxLoaded, "4"),
            Some("4".into())
        );
        assert_eq!(check_guard(VersionSetting::MaxLoaded, "0"), None);
        assert_eq!(check_guard(VersionSetting::MaxLoaded, "none"), None);
        assert_eq!(check_guard(VersionSetting::MaxConcurrentLoads, "99"), None);
        assert_eq!(
            check_guard(VersionSetting::MemoryBudget, "2GB"),
            Some("2GB".into())
        );
        assert_eq!(check_guard(VersionSetting::MemoryBudget, "lots"), None);
        assert_eq!(check_guard(VersionSetting::MemoryBudget, "1KB"), None);
        assert_eq!(
            check_guard(VersionSetting::AtCapacity, "refuse"),
            Some("refuse".into())
        );
        assert_eq!(check_guard(VersionSetting::AtCapacity, "maybe"), None);
        assert_eq!(
            check_guard(VersionSetting::WakeWait, "10s"),
            Some("10s".into())
        );
        assert_eq!(check_guard(VersionSetting::WakeWait, "1h"), None);
    }

    #[test]
    fn the_budget_is_shared_between_loaded_versions() {
        // 512 MB at 256 KB a view.
        assert_eq!(views_per_version(DEFAULT_MEMORY_BUDGET, 1), 2048);
        assert_eq!(views_per_version(DEFAULT_MEMORY_BUDGET, 4), 512);
        assert_eq!(parse_bytes("256kb"), Some(256 * 1024));
        assert_eq!(parse_bytes("1 GB"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_bytes("12"), Some(12));
        assert_eq!(parse_bytes("1TB"), None);
    }
}
