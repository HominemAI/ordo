//! Idempotency replay: the *same caller* retrying the *same request* gets the
//! stored response instead of a second execution.
//!
//! # Why the key is four-part and payload-bound (SH-17 / X-039)
//!
//! The cache used to key on `(op, header value)`. That is a cross-principal
//! read: an attacker who calls the op with a victim's `Idempotency-Key`
//! receives the victim's stored response **verbatim** — the written row,
//! post-defaults, sealed values included — because the response comes from the
//! cache and the executor, with its row-level guards, never runs. A replay
//! cache keyed below the principal is a guard bypass, not a convenience.
//!
//! So the key is [`ReplayKey`] = `(space, principal, op, key)`, and every
//! entry additionally stores a [`RequestFingerprint`] over the bound payload.
//! Three consequences, each deliberate:
//!
//! - **No cross-principal hit.** Another principal, or the same principal in
//!   another space, is a different key — a miss, and the executor runs under
//!   that caller's own guards.
//! - **No poisoning.** Pre-registering a key the victim's SDK will use lands
//!   in the *attacker's* keyspace; the victim's write executes normally.
//! - **No payload confusion.** Same key, different payload is
//!   [`ReplayClaim::Mismatch`] → `idempotency_mismatch` (409), never the
//!   first payload's response.
//!
//! # Why the key is *reserved*, not merely looked up (Z-012 / SH2-15)
//!
//! Lookup → execute → put is not idempotency. Two concurrent identical
//! requests both miss (neither has put yet), both execute, and two command
//! groups land — the exact double-write an idempotency key exists to prevent.
//! Retries are *concurrent* far more often than not: a client timeout that
//! fires while the first attempt is still running is the canonical case.
//!
//! So there is no bare lookup on this trait. [`ReplayCache::claim`] is a
//! single atomic step that either **reserves** the key (returning
//! [`ReplayClaim::Reserved`], the caller's obligation to execute and then
//! [`ReplayCache::put`] or [`ReplayCache::abandon`]) or reports what the key
//! already holds: a finished response, a live reservation somebody else won,
//! or a payload mismatch. The loser of the race never executes; it waits for
//! the winner's outcome and replays it.
//!
//! **A reservation expires.** A winner that crashes mid-execution — process
//! killed, pod evicted — leaves a pending marker nobody will ever complete.
//! Without a timeout that key is dead forever, which converts a crash into a
//! permanent denial of service against one caller's retry. So a reservation
//! older than [`DEFAULT_RESERVATION_TIMEOUT_SECS`] is takeable by the next
//! caller: the promise degrades from "exactly once" to "at most twice, and
//! only after a crash", which is the honest bound and is stated rather than
//! hidden.
//!
//! # What the Postgres-backed implementation must do
//!
//! [`MemoryReplayCache`] is correct for a single node and **wrong for more
//! than one**: two nodes hold two caches, so a retry that lands elsewhere
//! re-executes and the "exactly once" promise degrades to "at least once"
//! silently. A production implementation is Postgres-backed (the same store
//! that holds the write, so the entry commits in the write's transaction), and
//! it must **encrypt stored responses at rest under the space key** — a replay
//! cache is a plaintext mirror of every write response, which is exactly the
//! thing the rest of the platform refuses to keep. It also bounds itself: this
//! implementation carries a TTL and an entry cap because an unbounded cache of
//! plaintext responses is a retention decision nobody made.
//!
//! Its `claim` is a database primitive, not application logic:
//!
//! - **Reservation.** `INSERT INTO idempotency (space, principal, op, key,
//!   fingerprint, state, reserved_at) VALUES (…, 'pending', now())
//!   ON CONFLICT (space, principal, op, key) DO NOTHING RETURNING id`. A
//!   returned row means this caller won and holds the reservation. No row
//!   returned means somebody else holds the key: `SELECT` it and answer
//!   `Hit` / `InFlight` / `Mismatch` from `state` and `fingerprint`. The
//!   uniqueness constraint is the mutual exclusion — never a `SELECT` followed
//!   by an `INSERT`, which is the same race one layer down.
//! - **Stale takeover.** The reservation timeout is a predicate on the
//!   conflicting row, applied in the same statement so the takeover is atomic
//!   too: `… ON CONFLICT (…) DO UPDATE SET reserved_at = now(),
//!   fingerprint = EXCLUDED.fingerprint WHERE idempotency.state = 'pending'
//!   AND idempotency.reserved_at < now() - interval '60 seconds' RETURNING id`.
//!   A returned row is the takeover; no row means the holder is still live.
//! - **The loser.** It does not execute, ever. It waits on the winner —
//!   `LISTEN`/`NOTIFY` on the key, or a bounded poll of the same `SELECT` —
//!   and then replays the stored response. If its wait budget elapses before
//!   the winner commits, it answers `idempotency_in_flight` (409) and the
//!   client retries; it must not fall through to execution, because that is
//!   the double-write again.
//! - **Completion.** The winner's `put` is `UPDATE … SET state = 'done',
//!   response = …, stored_at = now()` **in the same transaction as the domain
//!   write and the chain append**. Committing the row without the response, or
//!   the response without the row, reintroduces the hole from the other side.
//!   A non-storable failure `DELETE`s the reservation (`abandon`) rather than
//!   leaving the key parked until the timeout.
//!
//!   **Narrowed by ruling (2026-09-06, `docs/plan-idempotency.md` §2):** what
//!   rides the write's transaction is the *completion record*, not the sealed
//!   response body. `ordo_exec`'s `$replay` journal claims and completes the
//!   key inside the effect's own transaction, so a committed write is never
//!   re-executed; the body is sealed and stored after commit, because sealing
//!   takes a key-provider release that would otherwise have to succeed before
//!   any write could commit. A crash between the two loses the body alone, and
//!   the retry is told so by name: `idempotency_applied` (409, terminal). This
//!   trait's shape — no transaction parameter — is therefore the intended one
//!   rather than a limitation waiting to be lifted.
//!
//! # Why some bindings outlive their response (Z-018 / BJ3-017)
//!
//! For a **creating** op the key→payload binding is not a transport
//! convenience, it is the create-once guarantee. A stored response expires
//! after [`DEFAULT_TTL_SECS`]; the row it created does not. If the binding
//! expired with the response, the same key replayed a day later with an
//! *edited* payload would miss the cache, execute, and silently converge onto
//! the existing row — an update nobody asked for, with no `rev` and therefore
//! no CAS to catch it.
//!
//! So [`KeyBinding::Permanent`] records the fingerprint separately from the
//! response and never expires it: same key + different payload is
//! [`ReplayClaim::Mismatch`] forever, whatever became of the cached response.
//! In Postgres this binding is not a cache at all — it is a column on the
//! created row (`created_by_idempotency_key` + `created_payload_fingerprint`,
//! unique per `(space, principal, op, key)`), so it lives and dies with the
//! row it created. The in-memory map is bounded by the entry cap like
//! everything else here, and eviction is exactly where this implementation
//! stops being able to keep the promise — stated, not hidden.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::page::PageCursor;
use crate::refine::Refinement;
use crate::spec::BoundParams;

/// Default entry lifetime: 24h, the window an SDK retry plausibly spans.
pub const DEFAULT_TTL_SECS: u64 = 24 * 60 * 60;

/// How long a reservation may be held before another caller may take it over.
/// Long enough that no honest execution is ever displaced, short enough that a
/// crashed winner does not park a key for a day.
pub const DEFAULT_RESERVATION_TIMEOUT_SECS: u64 = 60;

/// Default entry cap. Eviction is oldest-first; a dropped entry costs a
/// re-execution, never a wrong answer.
pub const DEFAULT_MAX_ENTRIES: usize = 10_000;

/// Everything that must match before a stored response may be handed back.
/// Space and principal are *part of the key*, not a check after the fact —
/// there is no code path on which a lookup can cross either boundary.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReplayKey {
    pub space: String,
    pub principal: String,
    pub op: String,
    pub key: String,
}

/// SHA-256 over the request's canonical payload: space, op, op version hash,
/// bound params, refinement, and cursor. Bound params are used rather than the
/// raw body so that two spellings of the same request (query vs body, key
/// order) fingerprint identically — the retry story is about the *request*,
/// not its transport. The op's version hash rides along so a key reused across
/// a redeploy is a loud mismatch rather than a response shaped for a schema
/// that no longer exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestFingerprint(pub [u8; 32]);

impl RequestFingerprint {
    /// Hex, for a durable journal that stores text rather than bytes.
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn compute(
        space: &str,
        op: &str,
        version_hash: &str,
        bound: &BoundParams,
        refine: Option<&Refinement>,
        page: Option<&PageCursor>,
    ) -> RequestFingerprint {
        let mut hasher = Sha256::new();
        hasher.update(b"ordo.idempotency.fingerprint.v1");
        for part in [space, op, version_hash] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        // `serde_json::Map` is a `BTreeMap`, so serialization is key-sorted and
        // therefore canonical; the same holds for nested objects.
        let params = Value::Object(bound.0.clone());
        hasher.update(serde_json::to_vec(&params).expect("bound params serialize"));
        match refine {
            None => hasher.update([0u8]),
            Some(r) => {
                hasher.update([1u8]);
                hasher.update(serde_json::to_vec(r).expect("refinement serializes"));
            }
        }
        match page {
            None => hasher.update([0u8]),
            Some(c) => {
                hasher.update([1u8]);
                hasher.update(serde_json::to_vec(&c.keys).expect("cursor keys serialize"));
            }
        }
        RequestFingerprint(hasher.finalize().into())
    }
}

/// How long the key→payload binding must outlive the stored response.
///
/// The distinction is Z-018: for an ordinary write the binding is a transport
/// convenience and may lapse with the response, but for a **creating** op it
/// is the create-once guarantee and outliving the response is the whole point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyBinding {
    /// Ordinary write or delete: the binding lives exactly as long as the
    /// stored response does.
    UntilExpiry,
    /// A creating op: the key is bound to its payload for good. Same key,
    /// different payload is a mismatch even after the response expired —
    /// never a re-execution that silently converges the created row.
    Permanent,
}

/// A finished response worth replaying: successes and executor conflict
/// outcomes (they *are* the op's result). Validation errors, `not_found`,
/// `access_guard`, and `internal` never land here — a retry re-attempts those
/// against current state.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredResponse {
    pub status: u16,
    pub body: Value,
}

/// What a claim found. The four outcomes are distinct on purpose: a mismatch
/// must never degrade into a reservation (that re-executes a request the
/// caller believed was idempotent) nor into a hit (that answers the wrong
/// question with a 200), and an in-flight reservation must never degrade into
/// a reservation of its own (that is the double execution).
#[derive(Clone, Debug, PartialEq)]
pub enum ReplayClaim {
    /// The key was free and is now **reserved by this caller**. Execute, then
    /// [`ReplayCache::put`] the outcome — or [`ReplayCache::abandon`] the
    /// reservation if the outcome is not storable.
    Reserved,
    /// Same key, same payload, finished — replay verbatim.
    Hit(StoredResponse),
    /// Same key, same payload, but another caller holds a live reservation and
    /// has not stored its outcome yet. Wait for it; do not execute.
    InFlight,
    /// Same key, different payload — `idempotency_mismatch` (409).
    Mismatch,
}

/// # Why this trait is async
///
/// It was synchronous, which was honest while the only implementation was a
/// `Mutex<HashMap>`. A **durable** cache is a database round trip, and the
/// door calls `claim` from inside an async request handler — so a blocking
/// implementation would have to `block_on` on a runtime worker (a panic) or
/// `block_in_place` (a parked thread on every idempotent write). Neither is
/// something a caller should have to know about an implementation, so the
/// contract moved rather than each implementation smuggling a runtime.
///
/// `#[async_trait]` rather than a native `async fn` in the trait: the door
/// holds `Arc<dyn ReplayCache>`, and native async fns are not dyn-compatible.
#[async_trait::async_trait]
pub trait ReplayCache: Send + Sync {
    /// Reserve the key, or report what it already holds — **one atomic step**.
    /// There is deliberately no non-reserving lookup on this trait: a caller
    /// who could look first and reserve later is the Z-012 race again.
    async fn claim(
        &self,
        key: &ReplayKey,
        fingerprint: &RequestFingerprint,
        binding: KeyBinding,
    ) -> ReplayClaim;

    /// Complete the reservation with the outcome to replay.
    /// `#[must_use]` because this returns a future once the trait is async,
    /// and dropping it is silent: the reservation is never completed, so the
    /// caller's own retry sees a live reservation and answers
    /// `idempotency_in_flight` forever. That is exactly what happened while
    /// this trait was being converted, and it compiled.
    #[must_use = "the reservation is only completed when this future is awaited"]
    async fn put(
        &self,
        key: ReplayKey,
        fingerprint: RequestFingerprint,
        response: StoredResponse,
        binding: KeyBinding,
    );

    /// Release a reservation whose outcome is **not** storable (validation
    /// noise, `not_found`, `access_guard`, `internal`): the retry must re-run
    /// against current state, so the key must be free immediately rather than
    /// parked until the reservation timeout.
    #[must_use = "the reservation is only released when this future is awaited"]
    async fn abandon(&self, key: &ReplayKey);
}

/// One key's state. A reservation and a finished response are the same slot at
/// two moments, so the map cannot hold both and no caller can see a key as
/// simultaneously free and taken.
enum Slot {
    /// Reserved; execution in flight. Ages out at the reservation timeout so a
    /// crashed winner cannot hold the key forever.
    Pending {
        fingerprint: RequestFingerprint,
        reserved_at: u64,
    },
    /// Finished; ages out at the TTL.
    Done {
        fingerprint: RequestFingerprint,
        response: StoredResponse,
        stored_at: u64,
    },
}

#[derive(Default)]
struct Inner {
    slots: HashMap<ReplayKey, Slot>,
    /// Insertion order, for oldest-first eviction at the cap.
    order: VecDeque<ReplayKey>,
    /// Z-018: key → payload for creating ops. Never expires with the response;
    /// bounded only by the entry cap.
    bindings: HashMap<ReplayKey, RequestFingerprint>,
    /// Insertion order for `bindings`.
    binding_order: VecDeque<ReplayKey>,
}

/// In-memory implementation for a single node. See the module docs for why a
/// multi-node deployment needs the Postgres-backed one instead, and for what
/// that implementation must do to make `claim` atomic.
pub struct MemoryReplayCache {
    inner: Mutex<Inner>,
    ttl_secs: u64,
    reservation_timeout_secs: u64,
    max_entries: usize,
    /// Injectable clock — seconds since the epoch. Tests drive TTL expiry
    /// without sleeping; production reads the system clock.
    clock: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl Default for MemoryReplayCache {
    fn default() -> Self {
        MemoryReplayCache::new()
    }
}

impl MemoryReplayCache {
    pub fn new() -> Self {
        MemoryReplayCache::with_bounds(DEFAULT_TTL_SECS, DEFAULT_MAX_ENTRIES)
    }

    pub fn with_bounds(ttl_secs: u64, max_entries: usize) -> Self {
        MemoryReplayCache {
            inner: Mutex::new(Inner::default()),
            ttl_secs,
            reservation_timeout_secs: DEFAULT_RESERVATION_TIMEOUT_SECS,
            max_entries: max_entries.max(1),
            clock: Box::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs())
            }),
        }
    }

    /// Narrow (or widen) how long a reservation survives without completion.
    /// A deployment whose slowest op outlives the default must raise this, or
    /// a second caller will take over a reservation that is still live.
    pub fn with_reservation_timeout(mut self, secs: u64) -> Self {
        self.reservation_timeout_secs = secs;
        self
    }

    pub fn reservation_timeout_secs(&self) -> u64 {
        self.reservation_timeout_secs
    }

    /// Replace the clock (tests only in practice — the platform has one clock).
    pub fn with_clock(mut self, clock: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// Live slot count — reservations included — after dropping anything past
    /// its deadline.
    pub fn len(&self) -> usize {
        let now = (self.clock)();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        self.purge_expired(&mut inner, now);
        inner.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop every slot past its deadline: responses at the TTL, reservations
    /// at the reservation timeout. This is the one place a crashed winner's
    /// marker stops being the authority for its key.
    fn purge_expired(&self, inner: &mut Inner, now: u64) {
        let Inner { slots, order, .. } = inner;
        slots.retain(|_, slot| match slot {
            Slot::Pending { reserved_at, .. } => {
                now.saturating_sub(*reserved_at) < self.reservation_timeout_secs
            }
            Slot::Done { stored_at, .. } => now.saturating_sub(*stored_at) < self.ttl_secs,
        });
        order.retain(|k| slots.contains_key(k));
    }

    /// Evict oldest-first until one more slot fits under the cap.
    fn make_room(inner: &mut Inner, max_entries: usize) {
        while inner.slots.len() >= max_entries {
            match inner.order.pop_front() {
                Some(oldest) => {
                    inner.slots.remove(&oldest);
                }
                None => break,
            }
        }
    }
}

#[async_trait::async_trait]
impl ReplayCache for MemoryReplayCache {
    async fn claim(
        &self,
        key: &ReplayKey,
        fingerprint: &RequestFingerprint,
        _binding: KeyBinding,
    ) -> ReplayClaim {
        let now = (self.clock)();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Expired responses and stale reservations both stop being the
        // authority here, which is what keeps a crashed winner from parking
        // the key: the next caller purges the marker and reserves.
        self.purge_expired(&mut inner, now);

        // Z-018 first: the permanent binding outranks the (possibly expired)
        // response. A creating op's key is bound to its payload for good, so a
        // changed payload is a mismatch even when there is nothing to replay.
        if inner
            .bindings
            .get(key)
            .is_some_and(|bound| bound != fingerprint)
        {
            return ReplayClaim::Mismatch;
        }

        match inner.slots.get(key) {
            Some(Slot::Done {
                fingerprint: stored,
                response,
                ..
            }) => {
                if stored != fingerprint {
                    ReplayClaim::Mismatch
                } else {
                    ReplayClaim::Hit(response.clone())
                }
            }
            Some(Slot::Pending {
                fingerprint: reserved,
                ..
            }) => {
                if reserved != fingerprint {
                    ReplayClaim::Mismatch
                } else {
                    ReplayClaim::InFlight
                }
            }
            None => {
                Self::make_room(&mut inner, self.max_entries);
                inner.slots.insert(
                    key.clone(),
                    Slot::Pending {
                        fingerprint: *fingerprint,
                        reserved_at: now,
                    },
                );
                inner.order.push_back(key.clone());
                ReplayClaim::Reserved
            }
        }
    }

    async fn put(
        &self,
        key: ReplayKey,
        fingerprint: RequestFingerprint,
        response: StoredResponse,
        binding: KeyBinding,
    ) {
        let now = (self.clock)();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        self.purge_expired(&mut inner, now);
        if !inner.slots.contains_key(&key) {
            Self::make_room(&mut inner, self.max_entries);
            inner.order.push_back(key.clone());
        }
        inner.slots.insert(
            key.clone(),
            Slot::Done {
                fingerprint,
                response,
                stored_at: now,
            },
        );
        if binding == KeyBinding::Permanent && !inner.bindings.contains_key(&key) {
            while inner.bindings.len() >= self.max_entries {
                match inner.binding_order.pop_front() {
                    Some(oldest) => {
                        inner.bindings.remove(&oldest);
                    }
                    None => break,
                }
            }
            inner.bindings.insert(key.clone(), fingerprint);
            inner.binding_order.push_back(key);
        }
    }

    async fn abandon(&self, key: &ReplayKey) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Only a reservation is released. A finished entry is somebody's
        // answer and is never dropped by a later caller's failure.
        if matches!(inner.slots.get(key), Some(Slot::Pending { .. })) {
            inner.slots.remove(key);
            inner.order.retain(|k| k != key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    fn key(principal: &str) -> ReplayKey {
        ReplayKey {
            space: "spc_01".into(),
            principal: principal.into(),
            op: "addNote".into(),
            key: "k-123".into(),
        }
    }

    fn fp(byte: u8) -> RequestFingerprint {
        RequestFingerprint([byte; 32])
    }

    fn response() -> StoredResponse {
        StoredResponse {
            status: 200,
            body: Value::String("row".into()),
        }
    }

    /// Store a finished entry the way a winner would: reserve, then put.
    async fn store(cache: &MemoryReplayCache, k: ReplayKey, f: RequestFingerprint, b: KeyBinding) {
        assert_eq!(cache.claim(&k, &f, b).await, ReplayClaim::Reserved);
        cache.put(k, f, response(), b).await;
    }

    #[tokio::test]
    async fn lookup_is_scoped_to_space_and_principal() {
        let cache = MemoryReplayCache::new();
        store(&cache, key("usr_victim"), fp(1), KeyBinding::UntilExpiry).await;

        // Same op, same header value, different principal: not a hit — the
        // attacker merely reserves their own key. This is the cross-principal
        // replay that used to hand over the victim's response.
        assert_eq!(
            cache
                .claim(&key("usr_attacker"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Reserved
        );
        // Same principal, different space: also its own key.
        let other_space = ReplayKey {
            space: "spc_02".into(),
            ..key("usr_victim")
        };
        assert_eq!(
            cache
                .claim(&other_space, &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Reserved
        );
        // The owner replays.
        assert_eq!(
            cache
                .claim(&key("usr_victim"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Hit(response())
        );
    }

    #[tokio::test]
    async fn differing_fingerprint_is_a_mismatch_not_a_replay() {
        let cache = MemoryReplayCache::new();
        store(&cache, key("usr_a"), fp(1), KeyBinding::UntilExpiry).await;
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(2), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Mismatch
        );
    }

    #[tokio::test]
    async fn entries_expire_at_the_ttl() {
        let now = Arc::new(AtomicU64::new(1_000));
        let clock = now.clone();
        let cache =
            MemoryReplayCache::with_bounds(60, 8).with_clock(move || clock.load(Ordering::SeqCst));
        store(&cache, key("usr_a"), fp(1), KeyBinding::UntilExpiry).await;

        now.store(1_059, Ordering::SeqCst);
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Hit(response())
        );
        now.store(1_060, Ordering::SeqCst);
        // The promise lapsed, so the retry is an ordinary first attempt again
        // — and reserving is what a first attempt does.
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Reserved
        );
        cache.abandon(&key("usr_a")).await;
        assert_eq!(
            cache.len(),
            0,
            "expired entries are dropped, not merely hidden"
        );
    }

    #[tokio::test]
    async fn entry_count_is_bounded_oldest_first() {
        let cache = MemoryReplayCache::with_bounds(DEFAULT_TTL_SECS, 3);
        for i in 0..5 {
            let mut k = key("usr_a");
            k.key = format!("k-{i}");
            store(&cache, k, fp(1), KeyBinding::UntilExpiry).await;
        }
        assert_eq!(cache.len(), 3);
        let mut oldest = key("usr_a");
        oldest.key = "k-0".into();
        assert_eq!(
            cache.claim(&oldest, &fp(1), KeyBinding::UntilExpiry).await,
            ReplayClaim::Reserved
        );
        let mut newest = key("usr_a");
        newest.key = "k-4".into();
        assert_eq!(
            cache.claim(&newest, &fp(1), KeyBinding::UntilExpiry).await,
            ReplayClaim::Hit(response())
        );
    }

    #[tokio::test]
    async fn fingerprint_covers_params_refinement_and_version() {
        let params = |body: &str| {
            let mut m = serde_json::Map::new();
            m.insert("body".into(), Value::String(body.into()));
            BoundParams(m)
        };
        let base = RequestFingerprint::compute("spc_01", "addNote", "v1", &params("a"), None, None);
        assert_eq!(
            base,
            RequestFingerprint::compute("spc_01", "addNote", "v1", &params("a"), None, None),
            "the same request fingerprints identically"
        );
        for other in [
            RequestFingerprint::compute("spc_02", "addNote", "v1", &params("a"), None, None),
            RequestFingerprint::compute("spc_01", "addNote", "v2", &params("a"), None, None),
            RequestFingerprint::compute("spc_01", "addNote", "v1", &params("b"), None, None),
            RequestFingerprint::compute(
                "spc_01",
                "addNote",
                "v1",
                &params("a"),
                Some(&Refinement::default()),
                None,
            ),
        ] {
            assert_ne!(base, other);
        }
    }

    // ── Z-012: the reservation ──

    #[tokio::test]
    async fn first_claim_reserves_and_the_second_is_not_a_second_execution() {
        let cache = MemoryReplayCache::new();
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Reserved,
            "the winner reserves before executing"
        );
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::InFlight,
            "the loser must wait, not reserve a second execution"
        );
    }

    /// The race, run for real: N threads claim the same key at the same
    /// instant and exactly one may reserve it.
    #[tokio::test]
    async fn concurrent_claims_reserve_exactly_once() {
        const THREADS: usize = 16;
        let cache = Arc::new(MemoryReplayCache::new());
        let handle = tokio::runtime::Handle::current();
        let barrier = Arc::new(Barrier::new(THREADS));
        let reserved = Arc::new(AtomicUsize::new(0));
        let in_flight = Arc::new(AtomicUsize::new(0));

        std::thread::scope(|scope| {
            for _ in 0..THREADS {
                let (cache, barrier) = (cache.clone(), barrier.clone());
                let handle = handle.clone();
                let (reserved, in_flight) = (reserved.clone(), in_flight.clone());
                scope.spawn(move || {
                    barrier.wait();
                    // A plain OS thread, not a runtime worker, so blocking on
                    // the handle is sound here — and it is what real
                    // concurrency looks like to the cache: N independent
                    // arrivals with no ordering between them.
                    let claim = handle.block_on(cache.claim(
                        &key("usr_a"),
                        &fp(1),
                        KeyBinding::UntilExpiry,
                    ));
                    match claim {
                        ReplayClaim::Reserved => reserved.fetch_add(1, Ordering::SeqCst),
                        ReplayClaim::InFlight => in_flight.fetch_add(1, Ordering::SeqCst),
                        other => panic!("unexpected claim outcome: {other:?}"),
                    };
                });
            }
        });

        assert_eq!(
            reserved.load(Ordering::SeqCst),
            1,
            "exactly one caller executes"
        );
        assert_eq!(in_flight.load(Ordering::SeqCst), THREADS - 1);
    }

    #[tokio::test]
    async fn the_loser_replays_the_winners_response_once_it_lands() {
        let cache = MemoryReplayCache::new();
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Reserved
        );
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::InFlight
        );
        cache
            .put(key("usr_a"), fp(1), response(), KeyBinding::UntilExpiry)
            .await;
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Hit(response()),
            "the loser gets the winner's answer, not its own execution"
        );
    }

    #[tokio::test]
    async fn a_reservation_with_a_different_payload_is_a_mismatch_not_a_wait() {
        let cache = MemoryReplayCache::new();
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Reserved
        );
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(2), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Mismatch,
            "waiting for a response to a different question answers the wrong one"
        );
    }

    #[tokio::test]
    async fn a_crashed_winners_reservation_does_not_hold_the_key_forever() {
        let now = Arc::new(AtomicU64::new(1_000));
        let clock = now.clone();
        let cache = MemoryReplayCache::with_bounds(DEFAULT_TTL_SECS, 8)
            .with_reservation_timeout(30)
            .with_clock(move || clock.load(Ordering::SeqCst));

        // The winner reserves and never returns.
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Reserved
        );
        now.store(1_029, Ordering::SeqCst);
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::InFlight,
            "inside the timeout the holder is still presumed live"
        );
        now.store(1_030, Ordering::SeqCst);
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Reserved,
            "a stale reservation is takeable — a crash must not park the key"
        );
    }

    #[tokio::test]
    async fn abandon_frees_the_key_immediately_but_never_a_stored_answer() {
        let cache = MemoryReplayCache::new();
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Reserved
        );
        cache.abandon(&key("usr_a")).await;
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Reserved,
            "an unstorable outcome releases the key rather than parking it"
        );
        cache
            .put(key("usr_a"), fp(1), response(), KeyBinding::UntilExpiry)
            .await;
        cache.abandon(&key("usr_a")).await;
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Hit(response()),
            "abandon releases reservations, never a finished answer"
        );
    }

    // ── Z-018: create-once ──

    #[tokio::test]
    async fn a_creating_keys_payload_binding_outlives_its_response() {
        let now = Arc::new(AtomicU64::new(1_000));
        let clock = now.clone();
        let cache =
            MemoryReplayCache::with_bounds(60, 8).with_clock(move || clock.load(Ordering::SeqCst));
        store(&cache, key("usr_a"), fp(1), KeyBinding::Permanent).await;

        // Response expired: nothing left to replay.
        now.store(2_000, Ordering::SeqCst);
        // Same key, same payload — an honest retry, free to run again (the
        // create converges onto its own row, which is what it already did).
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(1), KeyBinding::Permanent)
                .await,
            ReplayClaim::Reserved
        );
        cache.abandon(&key("usr_a")).await;
        // Same key, **edited** payload — a silent converge onto the created
        // row. Refused, permanently, with no CAS available to catch it.
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(2), KeyBinding::Permanent)
                .await,
            ReplayClaim::Mismatch
        );
    }

    #[tokio::test]
    async fn a_non_creating_keys_binding_lapses_with_its_response() {
        let now = Arc::new(AtomicU64::new(1_000));
        let clock = now.clone();
        let cache =
            MemoryReplayCache::with_bounds(60, 8).with_clock(move || clock.load(Ordering::SeqCst));
        store(&cache, key("usr_a"), fp(1), KeyBinding::UntilExpiry).await;
        now.store(2_000, Ordering::SeqCst);
        assert_eq!(
            cache
                .claim(&key("usr_a"), &fp(2), KeyBinding::UntilExpiry)
                .await,
            ReplayClaim::Reserved,
            "no row was created, so there is nothing to converge onto"
        );
    }
}
