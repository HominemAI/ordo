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
//! After each read the node loads every pinned version it does not serve
//! yet ([`PinStream::load_wanted`]), one at a time
//! (`versions.max_concurrent_loads`, 1), and drops the ones no pin names.
//! Loads happen here and at boot only: no request ever starts one, so an
//! anonymous caller cannot make a node compile (V-29).
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
}

impl Read {
    /// Did the set of served versions change? The door's access engine
    /// and the routines are rebuilt when it did.
    pub fn versions_changed(&self) -> bool {
        !self.loaded.is_empty() || !self.dropped.is_empty()
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
    /// One load at a time on this node (`versions.max_concurrent_loads`).
    loading: tokio::sync::Mutex<()>,
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
        })
    }

    /// Read every pin again and take them whole, then load what they name.
    pub async fn reload(&self) -> Result<Read, VersionRefusal> {
        let before = self.deployment.pins();
        let (pins, seq) = Versions::on(&self.control).all_pins().await?;
        let mut map = PinMap {
            seq: seq.max(before.seq),
            ..PinMap::default()
        };
        for (space, version, epoch) in pins {
            // A pin read whole never moves back past one the stream gave.
            let epoch_held = before.by_space.get(&space).map(|(_, e)| *e);
            match epoch_held {
                Some(e) if e > epoch => {
                    let held = before.by_space[&space].clone();
                    map.by_space.insert(space, held);
                }
                _ => {
                    map.by_space.insert(space, (version, epoch));
                }
            }
        }
        let mut read = Read {
            moved: map
                .by_space
                .iter()
                .filter(|(s, p)| before.by_space.get(*s) != Some(*p))
                .map(|(s, _)| s.clone())
                .collect(),
            ..Read::default()
        };
        read.moved.sort();
        self.deployment.replace_pins(map);
        self.settle(&mut read).await;
        Ok(read)
    }

    /// Read the stream from the last row this node read, and take each
    /// change. Then load what the pins name.
    pub async fn tail(&self) -> Result<Read, VersionRefusal> {
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

    /// Load what the pins name and drop what they no longer do.
    async fn settle(&self, read: &mut Read) {
        // The registry's active version is the deploy channel's to load,
        // with its live check and the tenant catch-up; never this stream's.
        match Versions::on(&self.control).active_version().await {
            Ok(v) => self.deployment.follow(v),
            Err(e) if e.is_missing_table() => self.deployment.follow(None),
            Err(e) => eprintln!("pins: the active version was not read ({e})"),
        }
        let (loaded, refused) = self.load_wanted().await;
        read.loaded = loaded;
        read.refused = refused;
        read.dropped = self.deployment.retain_pinned();
    }

    /// Every version a pin names that this node does not serve.
    pub fn wanted(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .deployment
            .pins()
            .by_space
            .values()
            .map(|(v, _)| v.clone())
            .filter(|v| self.deployment.loaded(v).is_none())
            .filter(|v| self.deployment.following().as_deref() != Some(v.as_str()))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Load every version [`PinStream::wanted`] names, one at a time. The
    /// versions loaded, and the ones refused with why. A refused version
    /// keeps its tenants closed (`SpaceRefusal::Unserved`) and is tried
    /// again on the next read; the refusal is the deployment owner's alert.
    pub async fn load_wanted(&self) -> (Vec<String>, Vec<(String, String)>) {
        let _turn = self.loading.lock().await;
        let mut loaded = Vec::new();
        let mut refused = Vec::new();
        for version in self.wanted() {
            match deploy::load_pinned(
                &self.control,
                &self.cfg,
                &version,
                self.keys.clone(),
                self.space.clone(),
            )
            .await
            {
                Ok(v) if !v.custody_is(&self.keys) => refused.push((
                    version,
                    "it does not seal through this deployment's key provider".into(),
                )),
                Ok(v) => match self.deployment.install(v) {
                    Swapped::Installed { to, .. } => loaded.push(to),
                    Swapped::AlreadyServing(_) => {}
                    Swapped::Refused(why) => refused.push((version, why)),
                },
                Err(e) => refused.push((version, e.to_string())),
            }
        }
        (loaded, refused)
    }
}

/// One line per thing a read did, for the node's log.
pub fn log_lines(read: &Read) -> Vec<String> {
    let mut out = Vec::new();
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
    for (v, why) in &read.refused {
        out.push(format!(
            "versions: {v} is pinned and could not be loaded, so its tenants are closed: {why}"
        ));
    }
    out
}
