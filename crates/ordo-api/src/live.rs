//! One live stream per client, carrying many reads (checklist 27s).
//!
//! The owner, 2026-10-01: a stream per entity *"seems like it would be
//! pricey among tons of clients"*. So a client opens one stream and every
//! hook on its screen shares it.
//!
//! ## The wire
//!
//! ```text
//! GET    /live                    open a shared stream (SSE)
//!                                 first frame: event: ready   data: {"stream":"<id>"}
//!                                 then:        event: invalidate data: {"key":"<key>"}
//! POST   /live/{stream}           {"key":"r1","op":"listTasks","params":{"from":"2026-01-01"},
//!                                  "refine":{"where":[{"field":"status","op":"eq","value":"open"}]}}
//!                                 add one read; answers the read's first envelope
//! DELETE /live/{stream}/{key}     drop one read; 204
//! ```
//!
//! - **Each read passes the whole door alone.** A `POST` runs the token
//!   gate, the registry lookup, method enforcement, the space bind, the
//!   access check and the quota gate exactly as `GET /op/{name}?live=1`
//!   would, with the same headers. `params` is the query-string form.
//! - **A read may carry a refinement**, `refine`, in the `$refine` shape.
//!   It meets the one-shot path's gates and refines the first envelope.
//!   Signals stay per op: a refinement changes which rows an answer shows,
//!   not which writes can change it, so two reads of one op with different
//!   refinements hear the same writes.
//! - **A refusal answers the `POST` and nothing else.** The stream stays
//!   open and its other reads keep their signals.
//! - **A stream belongs to the token that opened it.** A `POST` or `DELETE`
//!   with another token, or for a stream that has closed, answers
//!   `not_found`. The two cases are one answer.
//! - **Signals are tagged.** An `invalidate` names the read's key and says
//!   nothing else, as on the single-read stream. A burst for one key while
//!   its last signal is still queued is one signal.
//! - **A stream with no reads closes** after [`LiveSettings::idle_close`],
//!   so a stream that never carries a read does not hold a connection.
//! - **Opening a stream takes a token the deployment issued.** The gate
//!   cannot name an op or a space yet, so it asks only whether the token
//!   verifies ([`crate::AccessChecker::token_known`]); a stranger holds no
//!   connection. Each read then meets the whole door.
//!
//! - **The door is asked again while a read is open.** Before each
//!   `invalidate` the read's door ([`crate::router::Door`]: the token gate
//!   and the access check) is asked again, and on every
//!   [`LiveSettings::recheck`] tick the token gate is asked once for the
//!   stream and the access check once per read. A refusal is the stream's
//!   last frame, `event: error`, carrying the error envelope and, when one
//!   read was refused, its `key`; the stream then closes. A client that
//!   reopens it meets each read's refusal on its `POST`.
//!
//! The idle window, the read cap and the re-check interval are
//! [`LiveSettings`]: a setting each, with a shipped default, read from
//! `ORDO_LIVE_IDLE_CLOSE_SECS`, `ORDO_LIVE_MAX_READS` and
//! `ORDO_LIVE_RECHECK_SECS` (`docs/settings.md` §11).
//!
//! `GET /op/{name}?live=1` stays as it was: one read, one stream. Sync
//! (`?live=1&sync=1`) is only there, because a sync's frames carry rows and
//! a shared stream carries signals only.

use std::collections::HashMap;
use std::convert::Infallible;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::response::sse::Event;
use futures::stream::{self, Stream};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use crate::router::{refusal_frame, Door};
use crate::spec::{OpSpec, Subscription};

/// The shipped cap on reads per stream. A screen with more widgets than
/// this opens a second stream.
pub const DEFAULT_MAX_READS_PER_STREAM: usize = 256;

/// The shipped idle window: how long a stream with no reads stays open.
pub const DEFAULT_IDLE_CLOSE_SECS: u64 = 60;

/// The longest read key the door accepts. Keys are the client's own labels,
/// and this is the wire's shape, not a policy.
pub const MAX_KEY_LEN: usize = 64;

/// The shipped re-check interval: how often a quiet feed asks the door
/// again, so a logout or a withdrawn delegation ends it with no write.
pub const DEFAULT_RECHECK_SECS: u64 = 30;
/// `ORDO_LIVE_RECHECK_SECS`: the bounds, 1 to 3,600.
pub const RECHECK_SECS_BOUNDS: (u64, u64) = (1, 3600);

/// The shared stream's two limits. Settings with shipped defaults, never
/// one fixed value: a deployment with wide screens raises the cap, one
/// with many idle tabs shortens the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSettings {
    /// The most reads one stream carries. `ORDO_LIVE_MAX_READS`.
    pub max_reads_per_stream: usize,
    /// How long a stream with no reads stays open. `ORDO_LIVE_IDLE_CLOSE_SECS`.
    pub idle_close: Duration,
    /// How often every long-lived read (`?live=1`, a sync, a read on
    /// `/live`) asks the door again when no write has made it.
    /// `ORDO_LIVE_RECHECK_SECS`.
    pub recheck: Duration,
}

impl Default for LiveSettings {
    fn default() -> Self {
        LiveSettings {
            max_reads_per_stream: DEFAULT_MAX_READS_PER_STREAM,
            idle_close: Duration::from_secs(DEFAULT_IDLE_CLOSE_SECS),
            recheck: Duration::from_secs(DEFAULT_RECHECK_SECS),
        }
    }
}

impl LiveSettings {
    /// The environment's values, the defaults for what it leaves unset.
    ///
    /// A value that is not a number, or a cap of zero, reads as unset
    /// here: the library's half of the bargain the host's
    /// `RuntimeConfig::from_env` strikes, which refuses the boot on the
    /// same variables so a deployment never runs on a default it did not
    /// choose, while a tool or test that sets nothing still works.
    pub fn from_env() -> LiveSettings {
        let var = |k: &str| std::env::var(k).ok();
        LiveSettings::parse(
            var("ORDO_LIVE_MAX_READS").as_deref(),
            var("ORDO_LIVE_IDLE_CLOSE_SECS").as_deref(),
            var("ORDO_LIVE_RECHECK_SECS").as_deref(),
        )
    }

    /// [`LiveSettings::from_env`] over given values, so the reading is
    /// testable without touching the process environment.
    pub fn parse(
        max_reads: Option<&str>,
        idle_close_secs: Option<&str>,
        recheck_secs: Option<&str>,
    ) -> LiveSettings {
        let num = |v: Option<&str>| v.and_then(|s| s.trim().parse::<u64>().ok());
        let d = LiveSettings::default();
        LiveSettings {
            max_reads_per_stream: num(max_reads)
                .filter(|n| *n > 0)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(d.max_reads_per_stream),
            idle_close: num(idle_close_secs).map_or(d.idle_close, Duration::from_secs),
            recheck: num(recheck_secs)
                .filter(|n| (RECHECK_SECS_BOUNDS.0..=RECHECK_SECS_BOUNDS.1).contains(n))
                .map_or(d.recheck, Duration::from_secs),
        }
    }

    /// How often an open stream asks whether it has idled out: a quarter
    /// of the window, and never so often that a short window spins.
    fn idle_tick(&self) -> Duration {
        (self.idle_close / 4).max(Duration::from_millis(100))
    }
}

/// Whether a key is one the door will tag signals with.
pub(crate) fn well_formed_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY_LEN
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

/// The open shared streams of one door. Cloned into every request's state.
/// `Default::default()` is an empty table under the shipped settings.
#[derive(Clone, Default)]
pub struct LiveStreams {
    inner: Arc<Mutex<Table>>,
    settings: Arc<LiveSettings>,
}

struct Table {
    seed: std::collections::hash_map::RandomState,
    seq: u64,
    streams: HashMap<String, StreamEntry>,
}

impl Default for Table {
    fn default() -> Self {
        Table {
            seed: std::collections::hash_map::RandomState::new(),
            seq: 0,
            streams: HashMap::new(),
        }
    }
}

struct StreamEntry {
    token: [u8; 32],
    tx: mpsc::UnboundedSender<Signal>,
    reads: HashMap<String, ReadEntry>,
    /// Bumped per subscribe, so a signal queued for a read that has since
    /// been dropped or replaced is not sent.
    generation: u64,
    empty_since: Option<Instant>,
}

struct ReadEntry {
    generation: u64,
    task: tokio::task::AbortHandle,
    /// What the door admitted the read for, asked again while it is open.
    op: Arc<OpSpec>,
    space: String,
}

impl Drop for ReadEntry {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Signal {
    key: String,
    generation: u64,
    queued: Arc<AtomicBool>,
}

/// Why a read could not be added to a stream.
pub(crate) enum AddRefused {
    /// The stream is gone, or belongs to another token.
    NoStream,
    /// The stream already carries [`LiveSettings::max_reads_per_stream`]
    /// reads.
    Full,
}

fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

impl LiveStreams {
    /// An empty table under `settings`.
    pub fn new(settings: LiveSettings) -> LiveStreams {
        LiveStreams {
            inner: Arc::default(),
            settings: Arc::new(settings),
        }
    }

    /// The settings this table runs under.
    pub fn settings(&self) -> &LiveSettings {
        &self.settings
    }

    /// Streams currently open. For tests and diagnostics.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().streams.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Open a stream for `token` and hand back its id and its frames.
    pub(crate) fn open(
        &self,
        token: &str,
        door: Door,
    ) -> (String, impl Stream<Item = Result<Event, Infallible>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let id = {
            let mut table = self.inner.lock().unwrap();
            table.seq += 1;
            let mut h = table.seed.build_hasher();
            h.write_u64(table.seq);
            let a = h.finish();
            let mut h = table.seed.build_hasher();
            h.write_u64(a ^ 0x9e37_79b9_7f4a_7c15);
            let b = h.finish();
            let id = format!("ls_{:016x}{:016x}{}", a, b, table.seq);
            table.streams.insert(
                id.clone(),
                StreamEntry {
                    token: token_hash(token),
                    tx,
                    reads: HashMap::new(),
                    generation: 0,
                    empty_since: Some(Instant::now()),
                },
            );
            id
        };
        let ready = Event::default()
            .event("ready")
            .data(serde_json::json!({ "stream": id }).to_string());
        let feed = Feed {
            id: id.clone(),
            streams: self.clone(),
            rx,
            tick: tokio::time::interval(self.settings.idle_tick()),
            recheck: crate::router::recheck_ticker(self.settings.recheck),
            door,
            first: Some(ready),
            refused: false,
        };
        let frames = stream::unfold(feed, |mut feed| async move {
            if feed.refused {
                return None;
            }
            if let Some(ready) = feed.first.take() {
                return Some((Ok(ready), feed));
            }
            loop {
                tokio::select! {
                    signal = feed.rx.recv() => {
                        let signal = signal?;
                        signal.queued.store(false, Ordering::SeqCst);
                        let Some((op, space)) = feed.streams.current(&feed.id, &signal) else {
                            continue;
                        };
                        if let Err(e) = feed.door.admit(&op, &space).await {
                            feed.refused = true;
                            return Some((Ok(refusal_frame(&e, Some(&signal.key))), feed));
                        }
                        let data = serde_json::json!({ "key": signal.key }).to_string();
                        return Some((Ok(Event::default().event("invalidate").data(data)), feed));
                    }
                    _ = feed.tick.tick() => {
                        if feed.streams.idle(&feed.id) {
                            return None;
                        }
                    }
                    _ = feed.recheck.tick() => {
                        if let Err(refused) = feed.recheck().await {
                            let (e, key) = *refused;
                            feed.refused = true;
                            return Some((Ok(refusal_frame(&e, key.as_deref())), feed));
                        }
                    }
                }
            }
        });
        (id, frames)
    }

    /// Whether `token` owns the open stream `id`.
    pub(crate) fn owns(&self, id: &str, token: &str) -> bool {
        let table = self.inner.lock().unwrap();
        table
            .streams
            .get(id)
            .is_some_and(|s| s.token == token_hash(token))
    }

    /// Whether `id` can take one more read under `key`.
    pub(crate) fn has_room(&self, id: &str, token: &str, key: &str) -> Result<(), AddRefused> {
        let table = self.inner.lock().unwrap();
        let s = table
            .streams
            .get(id)
            .filter(|s| s.token == token_hash(token))
            .ok_or(AddRefused::NoStream)?;
        if !s.reads.contains_key(key) && s.reads.len() >= self.settings.max_reads_per_stream {
            return Err(AddRefused::Full);
        }
        Ok(())
    }

    /// Carry `subscription`'s signals on stream `id`, tagged `key`. A read
    /// already under `key` is replaced.
    pub(crate) fn add(
        &self,
        id: &str,
        token: &str,
        key: &str,
        subscription: Subscription,
        op: &OpSpec,
        space: &str,
    ) -> Result<(), AddRefused> {
        let mut table = self.inner.lock().unwrap();
        let s = table
            .streams
            .get_mut(id)
            .filter(|s| s.token == token_hash(token))
            .ok_or(AddRefused::NoStream)?;
        if !s.reads.contains_key(key) && s.reads.len() >= self.settings.max_reads_per_stream {
            return Err(AddRefused::Full);
        }
        s.generation += 1;
        let generation = s.generation;
        let tx = s.tx.clone();
        let queued = Arc::new(AtomicBool::new(false));
        let key_owned = key.to_string();
        let task = tokio::spawn(async move {
            // The subscription is the task's state, so the bus keeps
            // listening for exactly as long as the read is on the stream.
            let mut sub = subscription;
            loop {
                match sub.receiver.recv().await {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        if queued.swap(true, Ordering::SeqCst) {
                            continue;
                        }
                        let signal = Signal {
                            key: key_owned.clone(),
                            generation,
                            queued: queued.clone(),
                        };
                        if tx.send(signal).is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        })
        .abort_handle();
        s.reads.insert(
            key.to_string(),
            ReadEntry {
                generation,
                task,
                op: Arc::new(op.clone()),
                space: space.to_string(),
            },
        );
        s.empty_since = None;
        Ok(())
    }

    /// Drop the read under `key`. `false` when the stream is gone or is
    /// another token's; a key the stream does not carry is not an error.
    pub(crate) fn remove(&self, id: &str, token: &str, key: &str) -> bool {
        let mut table = self.inner.lock().unwrap();
        let Some(s) = table
            .streams
            .get_mut(id)
            .filter(|s| s.token == token_hash(token))
        else {
            return false;
        };
        s.reads.remove(key);
        if s.reads.is_empty() && s.empty_since.is_none() {
            s.empty_since = Some(Instant::now());
        }
        true
    }

    /// The read a signal is for, when it is still the one on the stream:
    /// what its door admitted it for.
    fn current(&self, id: &str, signal: &Signal) -> Option<(Arc<OpSpec>, String)> {
        let table = self.inner.lock().unwrap();
        table
            .streams
            .get(id)
            .and_then(|s| s.reads.get(&signal.key))
            .filter(|r| r.generation == signal.generation)
            .map(|r| (r.op.clone(), r.space.clone()))
    }

    /// Every read on the stream, with what its door admitted it for.
    fn reads(&self, id: &str) -> Vec<(String, Arc<OpSpec>, String)> {
        let table = self.inner.lock().unwrap();
        table.streams.get(id).map_or_else(Vec::new, |s| {
            s.reads
                .iter()
                .map(|(k, r)| (k.clone(), r.op.clone(), r.space.clone()))
                .collect()
        })
    }

    fn idle(&self, id: &str) -> bool {
        let table = self.inner.lock().unwrap();
        match table.streams.get(id) {
            None => true,
            Some(s) => s
                .empty_since
                .is_some_and(|t| t.elapsed() >= self.settings.idle_close),
        }
    }

    fn close(&self, id: &str) {
        // Dropping the entry drops its reads, and each read aborts its task.
        let removed = self.inner.lock().unwrap().streams.remove(id);
        drop(removed);
    }
}

/// The stream's state. Dropped when the client goes away or the stream
/// idles out, and dropping it closes the stream at the door.
struct Feed {
    id: String,
    streams: LiveStreams,
    rx: mpsc::UnboundedReceiver<Signal>,
    tick: tokio::time::Interval,
    /// The door's idle re-check ([`LiveSettings::recheck`]).
    recheck: tokio::time::Interval,
    door: Door,
    first: Option<Event>,
    /// The door refused; the refusal has been sent and the stream ends.
    refused: bool,
}

impl Feed {
    /// The idle re-check: the token gate once for the stream (it answers
    /// about the token, whatever it reads), then the in-memory access check
    /// once per read. The error, and the key of the read refused when it
    /// was one read's.
    async fn recheck(&self) -> Result<(), Box<(crate::error::ApiError, Option<String>)>> {
        self.door.token("").await.map_err(|e| Box::new((e, None)))?;
        for (key, op, space) in self.streams.reads(&self.id) {
            self.door
                .access(&op, &space)
                .map_err(|e| Box::new((e, Some(key))))?;
        }
        Ok(())
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.streams.close(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_short_plain_labels() {
        assert!(well_formed_key("r1"));
        assert!(well_formed_key("std.tasks.list-2_x"));
        assert!(!well_formed_key(""));
        assert!(!well_formed_key("a b"));
        assert!(!well_formed_key("a/b"));
        assert!(!well_formed_key(&"k".repeat(MAX_KEY_LEN + 1)));
    }

    #[test]
    fn settings_read_the_environment_and_fall_back_to_the_defaults() {
        let d = LiveSettings::default();
        assert_eq!(d.max_reads_per_stream, DEFAULT_MAX_READS_PER_STREAM);
        assert_eq!(d.idle_close, Duration::from_secs(DEFAULT_IDLE_CLOSE_SECS));
        assert_eq!(d.recheck, Duration::from_secs(DEFAULT_RECHECK_SECS));
        assert_eq!(LiveSettings::parse(None, None, None), d);

        let set = LiveSettings::parse(Some(" 12 "), Some("5"), Some("2"));
        assert_eq!(set.max_reads_per_stream, 12);
        assert_eq!(set.idle_close, Duration::from_secs(5));
        assert_eq!(set.recheck, Duration::from_secs(2));

        // Not a number, a cap that admits no read, or a re-check out of its
        // bounds: the default, which the host refuses at boot on the same
        // values.
        assert_eq!(LiveSettings::parse(Some("many"), Some("1m"), Some("x")), d);
        assert_eq!(LiveSettings::parse(Some("0"), None, Some("0")), d);
        assert_eq!(LiveSettings::parse(None, None, Some("3601")), d);

        // The idle check never spins on a short window.
        assert_eq!(
            LiveSettings::parse(None, Some("0"), None).idle_tick(),
            Duration::from_millis(100)
        );
        assert_eq!(d.idle_tick(), Duration::from_secs(15));
    }
}
