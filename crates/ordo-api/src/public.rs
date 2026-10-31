//! The public read path's result cache, with exact invalidation
//! (`design-public-path.md` §3), and its CDN headers and purge hook (§3,
//! "CDN headers"; build steps 3 and 4).
//!
//! # What is cached
//!
//! The success envelope of a public read, as bytes, under
//! `(op version, op name, canonical bound parameters, page token)`. The space
//! is not in the key: a public read always runs in the deployment's space.
//! Every visitor is the reserved anonymous principal and a public op cannot
//! name `$caller` (the compiler refuses it), so the answer is the same for
//! everyone, which is what makes one entry serve them all.
//!
//! # When an entry is dropped
//!
//! - **A write commits to an entity the op reads.** The write path's
//!   `ordo.invalidate` notify reaches the bus, which names the op; every entry
//!   of that op is dropped, all argument variants at once. The cache
//!   subscribes **before** its first fetch and counts invalidations per op, so
//!   a write that lands while a miss is being fetched stops that answer from
//!   being stored.
//! - **A deploy.** [`PublicPath::drop_all`], called by the host on every
//!   swap. The version is in the key anyway; dropping frees the memory and
//!   keeps one rule.
//! - **A lost listener.** If the bus stops hearing the database, it cannot
//!   know what it missed: every entry is dropped, nothing is stored until it
//!   hears again, and the answers served meanwhile say `s-maxage=0` so no CDN
//!   extends the window.
//! - **Memory.** Least recently used first, past `ORDO_PUBLIC_CACHE_BYTES`.
//!
//! Nothing is learned from a signal: the cache only ever stores what the
//! executor answered, and a visitor's input reaches the key as typed, bound
//! parameters, never as content. There is no way to write into the cache
//! from outside.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use sha2::{Digest, Sha256};

use crate::spec::{Hearing, InvalidationBus};

/// The settings of the public path (`design-public-path.md` §9). Every value
/// is the app developer's or the deployment owner's to change; the defaults
/// are the design's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicSettings {
    /// `ORDO_PUBLIC_CACHE_BYTES`: the result cache per node. Default 256 MiB.
    /// `0` turns the cache off: every public read goes to the database.
    pub cache_bytes: usize,
    /// `ORDO_PUBLIC_CDN_S_MAXAGE`: how long a shared cache may hold a public
    /// answer, in seconds. Default 60 (ruled, `public-path-design` ask 2).
    pub s_maxage: u64,
    /// `ORDO_PUBLIC_CDN_SWR`: how long a shared cache may serve a stale
    /// answer while it refetches, in seconds. Default 300.
    pub swr: u64,
    /// `ORDO_PUBLIC_MISSES_PER_MINUTE`: how many cache misses one public op
    /// may send to the database per minute, on one node. Default 600. A
    /// miss over it is refused `429` and names this setting; `0` turns the
    /// cap off. Every distinct parameter value is a miss, so without it a
    /// visitor could make every call a database read (audit 2026-10-11, 27).
    pub misses_per_minute: u32,
    /// `ORDO_PUBLIC_WRITES_PER_HOUR`: how many writes one public write op
    /// may make per hour, on one node. Default 10000. A write over it is
    /// refused `429` and names this setting; `0` turns the cap off. Visitors
    /// have no token, so this bounds a flood without knowing who sent it
    /// (ruled, `public-data` ask 25).
    pub writes_per_hour: u32,
    /// `ORDO_PUBLIC_HUMAN_CHECK`: whether a public write must carry a human
    /// check that the provider confirms. Default on (ruled, `public-data`
    /// ask 25). Off is the app developer's choice, made by name.
    pub human_check: bool,
}

impl Default for PublicSettings {
    fn default() -> Self {
        PublicSettings {
            cache_bytes: 256 * 1024 * 1024,
            s_maxage: 60,
            swr: 300,
            misses_per_minute: 600,
            writes_per_hour: 10_000,
            human_check: true,
        }
    }
}

/// The header a public write carries its human-check token in
/// (`design-public-path.md` §5, "The human check").
pub const HUMAN_HEADER: &str = "x-ordo-human";

/// What a provider said about one human-check token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HumanVerdict {
    /// The provider confirmed the token.
    Pass,
    /// The provider refused the token: a bot, a replay, or an expired one.
    Fail,
    /// The provider could not be asked, with the reason for the log. The
    /// write is refused: an unchecked write is the thing the check exists
    /// to stop.
    Unavailable(String),
}

/// Checks a human-check token with its provider (`design-public-path.md`
/// §5). The host implements it with one HTTPS call through `ordo-egress`.
/// The widget runs in the page, but only this server-side call stops a bot
/// that skips the widget (ruled, `public-path-design` ask 3).
pub trait HumanVerifier: Send + Sync {
    /// The provider's name, for messages: `turnstile`, `hcaptcha`, `recaptcha`.
    fn provider(&self) -> &'static str;
    fn verify(&self, token: String) -> BoxFuture<'static, HumanVerdict>;
}

/// Counts public requests in the deployment's monitoring. Every public
/// request is counted, read or write, whatever the outcome. `op` is a
/// public op's name, or `-` for a tokenless call the door refused, so a
/// visitor cannot mint new series by guessing names.
pub trait PublicMonitor: Send + Sync {
    fn count(&self, op: &str, kind: &'static str, outcome: &'static str);
}

/// The public path's optional parts. `Default` is none of them.
#[derive(Clone, Default)]
pub struct PublicHooks {
    pub purge: Option<Arc<dyn PurgeHook>>,
    pub human: Option<Arc<dyn HumanVerifier>>,
    pub monitor: Option<Arc<dyn PublicMonitor>>,
}

/// Tells a CDN to drop what it holds under some tags (`design-public-path.md`
/// §3, "Telling caches to drop erased content"). The host implements it with
/// a POST through `ordo-egress`, so the destination is vetted. No hook means
/// no CDN, and nothing is posted.
pub trait PurgeHook: Send + Sync {
    /// Called once per drop, with every tag the drop covers. Fire and forget:
    /// the caller does not wait, and a failure is the hook's to report.
    fn purge(&self, tags: Vec<String>) -> BoxFuture<'static, ()>;
}

/// One cached answer.
struct Entry {
    op: String,
    body: Arc<Vec<u8>>,
    etag: String,
    /// The bus's loss count when the answer was fetched. A different count
    /// now means the listener was lost since, and the entry may be stale.
    losses: u64,
    /// The recency tick, for least-recently-used eviction.
    used: u64,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    /// Recency tick → key, the eviction order.
    order: BTreeMap<u64, String>,
    bytes: usize,
    tick: u64,
    /// Invalidations heard per op. A miss records the count before it
    /// fetches and stores only if the count has not moved.
    generation: HashMap<String, u64>,
    /// Ops whose invalidations this cache is subscribed to.
    subscribed: HashSet<String>,
    /// Misses per op in the current one-minute window: the window's start
    /// and the count so far.
    misses: HashMap<String, (std::time::Instant, u32)>,
    /// Writes per public write op in the current one-hour window.
    writes: HashMap<String, (std::time::Instant, u32)>,
    /// The entity tags of each op, as the bus reported them.
    tags: HashMap<String, Vec<String>>,
}

struct Inner {
    settings: PublicSettings,
    purge: Option<Arc<dyn PurgeHook>>,
    human: Option<Arc<dyn HumanVerifier>>,
    monitor: Option<Arc<dyn PublicMonitor>>,
    state: Mutex<State>,
    /// The bus is not hearing the database now: store nothing, and tell
    /// downstream caches not to hold what is served.
    deaf: AtomicBool,
    /// The bus's loss count, as last seen.
    losses: AtomicU64,
    /// Whether a watcher of the bus's hearing is installed.
    watching: AtomicBool,
}

/// The public path's cache. Cheap to clone; every clone is the same cache.
/// `Default` is the shipped settings with no purge hook.
#[derive(Clone)]
pub struct PublicPath {
    inner: Arc<Inner>,
}

impl Default for PublicPath {
    fn default() -> Self {
        PublicPath::new(PublicSettings::default(), None)
    }
}

impl std::fmt::Debug for PublicPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublicPath")
            .field("settings", &self.inner.settings)
            .field("purge", &self.inner.purge.is_some())
            .field("human", &self.inner.human.as_ref().map(|h| h.provider()))
            .field("monitor", &self.inner.monitor.is_some())
            .finish()
    }
}

/// What [`PublicPath::lookup`] found.
pub struct Hit {
    pub body: Arc<Vec<u8>>,
    pub etag: String,
}

/// A miss's ticket: what to compare when the answer comes back.
pub struct Ticket {
    key: String,
    op: String,
    generation: u64,
    losses: u64,
}

/// How a public answer is described to downstream caches.
pub struct Headers {
    pub cache_control: String,
    pub etag: String,
    /// `op:<name>, entity:<e1>, …` for `Cache-Tag`; the same tags joined by
    /// spaces for `Surrogate-Key`.
    pub tags: Vec<String>,
}

impl PublicPath {
    pub fn new(settings: PublicSettings, purge: Option<Arc<dyn PurgeHook>>) -> PublicPath {
        PublicPath::with_hooks(
            settings,
            PublicHooks {
                purge,
                ..Default::default()
            },
        )
    }

    pub fn with_hooks(settings: PublicSettings, hooks: PublicHooks) -> PublicPath {
        PublicPath {
            inner: Arc::new(Inner {
                settings,
                purge: hooks.purge,
                human: hooks.human,
                monitor: hooks.monitor,
                state: Mutex::new(State::default()),
                deaf: AtomicBool::new(false),
                losses: AtomicU64::new(0),
                watching: AtomicBool::new(false),
            }),
        }
    }

    pub fn settings(&self) -> &PublicSettings {
        &self.inner.settings
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The cache key: version, op, the bound parameters with sorted keys,
    /// and the page token. The same request spelled two ways is one entry.
    pub fn key(
        version: &str,
        op: &str,
        params: &serde_json::Map<String, serde_json::Value>,
        page: Option<&str>,
    ) -> String {
        // `serde_json::Map` without `preserve_order` is a `BTreeMap`, so the
        // serialisation is already sorted by key.
        let params = serde_json::to_string(params).unwrap_or_default();
        format!(
            "{version}\u{1f}{op}\u{1f}{params}\u{1f}{}",
            page.unwrap_or("")
        )
    }

    /// Subscribe to the op's invalidations, once per op, before its first
    /// fetch. Holds the bus subscription (and its listening lease) for the
    /// life of the process.
    pub async fn ensure_subscribed(&self, bus: &Arc<dyn InvalidationBus>, op: &str, space: &str) {
        {
            let mut st = self.state();
            if !st.subscribed.insert(op.to_string()) {
                return;
            }
            let tags = std::iter::once(format!("op:{op}"))
                .chain(
                    bus.entities_of(op)
                        .into_iter()
                        .map(|e| format!("entity:{e}")),
                )
                .collect();
            st.tags.insert(op.to_string(), tags);
        }
        let sub = bus.subscribe_in(op, space).await;
        if let Some(hearing) = sub.hearing.clone() {
            self.watch(hearing);
        }
        let this = self.clone();
        let op = op.to_string();
        tokio::spawn(async move {
            let mut sub = sub;
            loop {
                match sub.receiver.recv().await {
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        this.drop_op(&op);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // The bus is gone: nothing will tell this op's
                        // entries they are stale, so none may stay.
                        this.state().subscribed.remove(&op);
                        this.drop_all();
                        break;
                    }
                }
            }
        });
    }

    /// Watch the bus's hearing: a loss drops everything, and nothing is
    /// stored while deaf. One watcher; the public path is one space.
    fn watch(&self, mut hearing: tokio::sync::watch::Receiver<Hearing>) {
        if self.inner.watching.swap(true, Ordering::SeqCst) {
            return;
        }
        let now = *hearing.borrow();
        self.inner.losses.store(now.losses, Ordering::SeqCst);
        self.inner.deaf.store(!now.heard, Ordering::SeqCst);
        let this = self.clone();
        tokio::spawn(async move {
            while hearing.changed().await.is_ok() {
                let now = *hearing.borrow();
                this.inner.deaf.store(!now.heard, Ordering::SeqCst);
                let before = this.inner.losses.swap(now.losses, Ordering::SeqCst);
                if before != now.losses {
                    this.drop_all();
                }
            }
            this.inner.watching.store(false, Ordering::SeqCst);
        });
    }

    /// Is the bus hearing the database now?
    pub fn deaf(&self) -> bool {
        self.inner.deaf.load(Ordering::SeqCst)
    }

    /// Look a key up. A hit refreshes its recency. An entry from before a
    /// lost listener is not a hit, and its presence drops everything.
    pub fn lookup(&self, key: &str) -> Option<Hit> {
        if self.deaf() {
            return None;
        }
        let losses = self.inner.losses.load(Ordering::SeqCst);
        let mut st = self.state();
        let stale = st.entries.get(key).map(|e| e.losses != losses)?;
        if stale {
            drop(st);
            self.drop_all();
            return None;
        }
        st.tick += 1;
        let tick = st.tick;
        let entry = st.entries.get_mut(key)?;
        let old = std::mem::replace(&mut entry.used, tick);
        let hit = Hit {
            body: entry.body.clone(),
            etag: entry.etag.clone(),
        };
        st.order.remove(&old);
        st.order.insert(tick, key.to_string());
        Some(hit)
    }

    /// Count a miss of `op` against `ORDO_PUBLIC_MISSES_PER_MINUTE`, before
    /// fetching. `false` means the op has used its misses for this minute and
    /// the read must not reach the database.
    pub fn admit_miss(&self, op: &str) -> bool {
        self.admit_miss_at(op, std::time::Instant::now())
    }

    fn admit_miss_at(&self, op: &str, now: std::time::Instant) -> bool {
        let cap = self.inner.settings.misses_per_minute;
        if cap == 0 {
            return true;
        }
        let mut st = self.state();
        let slot = st.misses.entry(op.to_string()).or_insert((now, 0));
        if now.saturating_duration_since(slot.0) >= std::time::Duration::from_secs(60) {
            *slot = (now, 0);
        }
        if slot.1 >= cap {
            return false;
        }
        slot.1 += 1;
        true
    }

    /// Whether a public write op has room left this hour, without using it.
    /// Read before the human check, so a flood past the cap costs no call
    /// to the provider.
    pub fn write_room(&self, op: &str) -> bool {
        self.write_window(op, std::time::Instant::now(), false)
    }

    /// Count one write of `op` against `ORDO_PUBLIC_WRITES_PER_HOUR`. Called
    /// after the human check passes, so a bot's failed tokens never use up
    /// the hour and lock real visitors out. `false` means the hour is full.
    pub fn admit_write(&self, op: &str) -> bool {
        self.write_window(op, std::time::Instant::now(), true)
    }

    fn write_window(&self, op: &str, now: std::time::Instant, take: bool) -> bool {
        let cap = self.inner.settings.writes_per_hour;
        if cap == 0 {
            return true;
        }
        let mut st = self.state();
        let slot = st.writes.entry(op.to_string()).or_insert((now, 0));
        if now.saturating_duration_since(slot.0) >= std::time::Duration::from_secs(3600) {
            *slot = (now, 0);
        }
        if slot.1 >= cap {
            return false;
        }
        if take {
            slot.1 += 1;
        }
        true
    }

    /// The human-check provider, when one is configured.
    pub fn human(&self) -> Option<&Arc<dyn HumanVerifier>> {
        self.inner.human.as_ref()
    }

    /// Count one public request in the deployment's monitoring.
    pub fn count(&self, op: &str, kind: &'static str, outcome: &'static str) {
        if let Some(m) = &self.inner.monitor {
            m.count(op, kind, outcome);
        }
    }

    /// Take a ticket for a miss, before fetching.
    pub fn ticket(&self, key: String, op: &str) -> Ticket {
        let generation = self.state().generation.get(op).copied().unwrap_or(0);
        Ticket {
            key,
            op: op.to_string(),
            generation,
            losses: self.inner.losses.load(Ordering::SeqCst),
        }
    }

    /// Store a fetched answer, unless an invalidation, a lost listener or the
    /// size bound says not to. Returns the `ETag` either way.
    pub fn store(&self, ticket: Ticket, body: Vec<u8>) -> (Arc<Vec<u8>>, String) {
        let etag = etag_of(&body);
        let body = Arc::new(body);
        let cap = self.inner.settings.cache_bytes;
        let size = body.len() + ticket.key.len();
        let fresh = !self.deaf()
            && self.inner.losses.load(Ordering::SeqCst) == ticket.losses
            && size <= cap;
        if !fresh {
            return (body, etag);
        }
        let mut st = self.state();
        if st.generation.get(&ticket.op).copied().unwrap_or(0) != ticket.generation {
            return (body, etag);
        }
        if let Some(old) = st.entries.remove(&ticket.key) {
            st.order.remove(&old.used);
            st.bytes -= old.body.len() + ticket.key.len();
        }
        while st.bytes + size > cap {
            let Some((_, victim)) = st.order.pop_first() else {
                break;
            };
            if let Some(e) = st.entries.remove(&victim) {
                st.bytes -= e.body.len() + victim.len();
            }
        }
        st.tick += 1;
        let tick = st.tick;
        st.order.insert(tick, ticket.key.clone());
        st.bytes += size;
        st.entries.insert(
            ticket.key,
            Entry {
                op: ticket.op,
                body: body.clone(),
                etag: etag.clone(),
                losses: ticket.losses,
                used: tick,
            },
        );
        (body, etag)
    }

    /// Drop every entry of one op, all argument variants at once, and tell
    /// the CDN once.
    pub fn drop_op(&self, op: &str) {
        let tags = {
            let mut st = self.state();
            *st.generation.entry(op.to_string()).or_default() += 1;
            let keys: Vec<String> = st
                .entries
                .iter()
                .filter(|(_, e)| e.op == op)
                .map(|(k, _)| k.clone())
                .collect();
            for k in keys {
                if let Some(e) = st.entries.remove(&k) {
                    st.order.remove(&e.used);
                    st.bytes -= e.body.len() + k.len();
                }
            }
            st.tags.get(op).cloned().unwrap_or_default()
        };
        self.purge(tags);
    }

    /// Drop everything: a deploy, a lost listener, a closed bus. Tells the
    /// CDN about every op this node has served.
    pub fn drop_all(&self) {
        let tags = {
            let mut st = self.state();
            st.entries.clear();
            st.order.clear();
            st.bytes = 0;
            let ops: Vec<String> = st.subscribed.iter().cloned().collect();
            for op in &ops {
                *st.generation.entry(op.clone()).or_default() += 1;
            }
            let mut tags: Vec<String> = ops
                .iter()
                .flat_map(|op| st.tags.get(op).cloned().unwrap_or_default())
                .collect();
            tags.sort();
            tags.dedup();
            tags
        };
        self.purge(tags);
    }

    fn purge(&self, tags: Vec<String>) {
        if tags.is_empty() {
            return;
        }
        if let Some(hook) = &self.inner.purge {
            let fut = hook.purge(tags);
            match tokio::runtime::Handle::try_current() {
                Ok(rt) => {
                    rt.spawn(fut);
                }
                Err(_) => futures::executor::block_on(fut),
            }
        }
    }

    /// How many entries, and how many bytes, are held now.
    pub fn size(&self) -> (usize, usize) {
        let st = self.state();
        (st.entries.len(), st.bytes)
    }

    /// The headers a public answer carries (`design-public-path.md` §3).
    pub fn headers(&self, op: &str, etag: &str) -> Headers {
        let s = &self.inner.settings;
        let cache_control = if self.deaf() {
            // The listener is lost: what is served now could go stale with
            // no signal, so no shared cache may hold it.
            "public, max-age=0, s-maxage=0".to_string()
        } else {
            format!(
                "public, max-age=0, s-maxage={}, stale-while-revalidate={}",
                s.s_maxage, s.swr
            )
        };
        let tags = self
            .state()
            .tags
            .get(op)
            .cloned()
            .unwrap_or_else(|| vec![format!("op:{op}")]);
        Headers {
            cache_control,
            etag: etag.to_string(),
            tags,
        }
    }
}

/// `"<sha256 of the body>"`, quoted as an `ETag` is.
pub fn etag_of(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("\"{hex}\"")
}

/// Does `If-None-Match` name this `ETag`? `*` and a list are both read.
pub fn not_modified(if_none_match: Option<&str>, etag: &str) -> bool {
    let Some(v) = if_none_match else {
        return false;
    };
    v.split(',')
        .map(str::trim)
        .any(|t| t == "*" || t == etag || t.strip_prefix("W/") == Some(etag))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::MemoryBus;

    /// Records every purge, in order.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<Vec<String>>>);

    impl PurgeHook for Recorder {
        fn purge(&self, tags: Vec<String>) -> BoxFuture<'static, ()> {
            self.0.lock().unwrap().push(tags);
            Box::pin(async {})
        }
    }

    fn params(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn the_key_ignores_parameter_order() {
        let a = PublicPath::key(
            "v1",
            "op",
            &params(serde_json::json!({"a": 1, "b": 2})),
            None,
        );
        let b = PublicPath::key(
            "v1",
            "op",
            &params(serde_json::json!({"b": 2, "a": 1})),
            None,
        );
        assert_eq!(a, b);
        let c = PublicPath::key(
            "v2",
            "op",
            &params(serde_json::json!({"a": 1, "b": 2})),
            None,
        );
        assert_ne!(a, c, "the version is in the key");
    }

    #[tokio::test]
    async fn a_drop_posts_the_dropped_tags_once() {
        let hook = Arc::new(Recorder::default());
        let cache = PublicPath::new(PublicSettings::default(), Some(hook.clone()));
        let bus: Arc<dyn InvalidationBus> = Arc::new(MemoryBus::new());
        cache.ensure_subscribed(&bus, "pricing", "system").await;
        let t = cache.ticket("k".into(), "pricing");
        cache.store(t, b"{}".to_vec());
        cache.drop_op("pricing");
        assert_eq!(
            *hook.0.lock().unwrap(),
            vec![vec!["op:pricing".to_string()]]
        );
        assert_eq!(cache.size().0, 0);
    }

    #[tokio::test]
    async fn an_unset_hook_posts_nothing() {
        let cache = PublicPath::default();
        let t = cache.ticket("k".into(), "pricing");
        cache.store(t, b"{}".to_vec());
        cache.drop_op("pricing");
        cache.drop_all();
        assert_eq!(cache.size().0, 0);
    }

    #[test]
    fn an_invalidation_during_a_fetch_stops_the_store() {
        let cache = PublicPath::default();
        let t = cache.ticket("k".into(), "pricing");
        cache.drop_op("pricing");
        cache.store(t, b"stale".to_vec());
        assert!(
            cache.lookup("k").is_none(),
            "the answer may predate the write"
        );
    }

    #[test]
    fn the_least_recently_used_entry_goes_first() {
        let cache = PublicPath::new(
            PublicSettings {
                cache_bytes: 20,
                ..PublicSettings::default()
            },
            None,
        );
        cache.store(cache.ticket("a".into(), "x"), vec![0; 8]);
        cache.store(cache.ticket("b".into(), "x"), vec![0; 8]);
        assert!(cache.lookup("a").is_some());
        cache.store(cache.ticket("c".into(), "x"), vec![0; 8]);
        assert!(cache.lookup("b").is_none(), "b was least recently used");
        assert!(cache.lookup("a").is_some());
        assert!(cache.lookup("c").is_some());
    }

    #[test]
    fn misses_over_the_per_minute_cap_are_refused_until_the_minute_turns() {
        let cache = PublicPath::new(
            PublicSettings {
                misses_per_minute: 2,
                ..PublicSettings::default()
            },
            None,
        );
        let t0 = std::time::Instant::now();
        assert!(cache.admit_miss_at("pricing", t0));
        assert!(cache.admit_miss_at("pricing", t0));
        assert!(
            !cache.admit_miss_at("pricing", t0),
            "the third miss is over"
        );
        assert!(cache.admit_miss_at("other", t0), "the cap is per op");
        let later = t0 + std::time::Duration::from_secs(60);
        assert!(cache.admit_miss_at("pricing", later), "a new minute");
    }

    #[test]
    fn writes_over_the_hourly_cap_are_refused_until_the_hour_turns() {
        let p = PublicPath::new(
            PublicSettings {
                writes_per_hour: 2,
                ..Default::default()
            },
            None,
        );
        let t = std::time::Instant::now();
        // Looking does not use the hour.
        for _ in 0..5 {
            assert!(p.write_window("contact", t, false));
        }
        assert!(p.write_window("contact", t, true));
        assert!(p.write_window("contact", t, true));
        assert!(!p.write_window("contact", t, false));
        assert!(!p.write_window("contact", t, true));
        assert!(p.write_window("other", t, true), "per op");
        let later = t + std::time::Duration::from_secs(3600);
        assert!(p.write_window("contact", later, true));
    }

    #[test]
    fn a_zero_miss_cap_is_off() {
        let cache = PublicPath::new(
            PublicSettings {
                misses_per_minute: 0,
                ..PublicSettings::default()
            },
            None,
        );
        assert!((0..10_000).all(|_| cache.admit_miss("pricing")));
    }

    #[test]
    fn if_none_match_reads_lists_and_weak_tags() {
        let e = etag_of(b"x");
        assert!(not_modified(Some(&e), &e));
        assert!(not_modified(Some(&format!("\"other\", W/{e}")), &e));
        assert!(not_modified(Some("*"), &e));
        assert!(!not_modified(Some("\"other\""), &e));
        assert!(!not_modified(None, &e));
    }
}
