//! 8p — cross-node invalidation.
//!
//! `ordo-exec`'s write path already does the hard half correctly: inside
//! the write transaction it calls `pg_notify('ordo.invalidate', <entity>)`,
//! so the signal is delivered at commit and discarded on rollback, with no
//! outbox and no delivery bookkeeping. What was missing was the other end.
//! `MemoryBus` — a tokio broadcast channel — was the only
//! [`InvalidationBus`], so a `?live=1` subscriber on node A never saw node
//! B's writes. Not an error: silence, indistinguishable from "nothing
//! changed", on the one feature whose entire point is not being stale.
//!
//! [`PgInvalidationBus`] is the LISTEN side. A dedicated connection per
//! database (`ordo_exec::jobs::listen`), one forwarding task each, and an
//! index from entity name to the ops that read it.
//!
//! # Two things it deliberately does not do
//!
//! **It has no local shortcut.** A write on this very node publishes
//! nothing directly into the broadcast channel; it goes out through
//! `pg_notify` and comes back through the listener like everyone else's.
//! Postgres delivers a notification to every listening session including
//! sessions of the notifying backend, and the listener holds its own
//! connection, so the local path and the remote path are the *same* path.
//! Two paths would be two behaviours to keep in agreement, and the one
//! that only ever runs in production is the one that would rot.
//!
//! **It never carries data.** The payload is an entity name. Subscribers
//! learn *that* something they read may have changed and refetch through
//! the ordinary op under their own guards — so a signal about a row the
//! subscriber may not see discloses nothing but the entity's name, which
//! their own manifest already told them.
//!
//! # The index, and why guard entities are in it
//!
//! An op subscribes to the entities it reads. DL G16 settles that guard
//! walks are part of that set — "entities reached by a guard walk are part
//! of the op's inlined base-entity set: they appear in the scope manifest
//! and are subscribed by `?live`" — because a membership or acl write
//! changes which rows a guarded live query returns without touching the
//! rows themselves. So the index walks each op's sources, its callees'
//! sources (composition flattens), and each reached entity's guard.
//!
//! The index errs toward **over**-invalidation where it is unsure: a
//! spurious signal costs the client one refetch, a missed one leaves it
//! wrong indefinitely.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ordo_api::{
    Hearing, InvalidationBus, InvalidationEvent, MemoryBus, OpKind, Subscribing, Subscription,
};
use ordo_conformance::Fixture;
use ordo_exec::jobs::INVALIDATE_CHANNEL;
use ordo_exec::PoolConfig;
use ordo_schema::{GuardBody, Schema};
use ordo_syntax::GuardAtom;
use tokio::sync::broadcast;

/// entity name → the ops whose results that entity's rows can change.
pub type EntityIndex = BTreeMap<String, Vec<String>>;

/// The entity index the listener reads on every notification, swappable so
/// a deploy can install the new version's (`design-public-path.md` §3,
/// "Deploy"). Before this a bus kept the index it was built with for the
/// life of the process, so an op a deploy added was never invalidated.
pub type SharedIndex = Arc<std::sync::RwLock<Arc<EntityIndex>>>;

/// Build the index from the same compiler artifacts everything else here
/// derives from. Read ops only: a write op is not subscribable, so nothing
/// is ever published for one.
pub fn entity_index(f: &Fixture) -> EntityIndex {
    let mut index: EntityIndex = BTreeMap::new();
    for spec in crate::build_specs(f) {
        if spec.kind != OpKind::Read {
            continue;
        }
        for entity in subscribed_entities(f, &spec.name) {
            let ops = index.entry(entity).or_default();
            if !ops.contains(&spec.name) {
                ops.push(spec.name.clone());
            }
        }
    }
    index
}

/// Every entity a read op's result depends on: its own and its callees'
/// sources (the manifest's set), plus every entity each of those reaches
/// through its guard walk (G16).
pub fn subscribed_entities(f: &Fixture, op: &str) -> BTreeSet<String> {
    let mut base: Vec<String> = Vec::new();
    crate::collect_entities(f, op, &mut base, &mut Vec::new());
    let mut out: BTreeSet<String> = BTreeSet::new();
    for e in base {
        if out.insert(e.clone()) {
            guard_entities(&f.schema, &e, &mut out);
        }
    }
    out
}

/// Walk `entity`'s effective guard and add every entity it reaches.
///
/// The walk grammar is closed (DL G1): forward hops over ref fields, and
/// reverse collection steps naming an entity. Resolution mirrors
/// `ordo-schema`'s own `validate_walk` — a segment that is a ref field
/// moves to its target, a segment that names an entity moves there — with
/// one difference: where the validator refuses an unresolvable segment,
/// this stops following it. The validator has already run at load, so a
/// segment that does not resolve here is a shape this index does not
/// understand, and quietly widening is the wrong direction from missing.
fn guard_entities(schema: &Schema, entity: &str, out: &mut BTreeSet<String>) {
    let Some(e) = schema.entity(entity) else {
        return;
    };
    let Some(g) = e.guards.as_ref() else { return };
    // An inherited guard's walk text starts at the ancestor it came from;
    // `rebase` is the ownership path that carries it down. Follow the
    // rebase first so the walk starts where its author wrote it.
    let mut home = entity.to_string();
    for hop in &g.rebase {
        match step(schema, &home, hop) {
            Some(next) => {
                out.insert(next.clone());
                home = next;
            }
            None => return,
        }
    }
    for body in [&g.read, &g.write] {
        if let GuardBody::Walk(w) = body {
            walk_expr(schema, &home, w, out);
        }
    }
}

fn walk_expr(schema: &Schema, home: &str, w: &ordo_syntax::GuardExpr, out: &mut BTreeSet<String>) {
    for conj in &w.disjuncts {
        for atom in conj {
            match atom {
                GuardAtom::Cmp { path, .. } | GuardAtom::Bare { path, .. } => {
                    walk_path(schema, home, path, out);
                }
                GuardAtom::Step { path, inner, .. } => {
                    let landed = walk_path(schema, home, path, out);
                    walk_expr(schema, &landed, inner, out);
                }
            }
        }
    }
}

/// Follow a path from `home`, recording every entity it passes through.
/// Returns where it landed (the last entity reached), which is where a
/// reverse step's inner expression is rooted.
fn walk_path(
    schema: &Schema,
    home: &str,
    path: &[ordo_syntax::Ident],
    out: &mut BTreeSet<String>,
) -> String {
    let mut cur = home.to_string();
    for seg in path {
        match step(schema, &cur, &seg.name) {
            Some(next) => {
                out.insert(next.clone());
                cur = next;
            }
            // A scalar field, or a segment naming the arriving ref — the
            // path ends here for our purposes.
            None => break,
        }
    }
    cur
}

/// One segment: a ref field of `cur` moves to its target; a segment that
/// names an entity is a reverse collection step onto that entity.
fn step(schema: &Schema, cur: &str, seg: &str) -> Option<String> {
    if let Some(e) = schema.entity(cur) {
        if let Some(ordo_schema::FieldTy::Ref { target, .. }) = e.field(seg).map(|f| &f.ty) {
            return Some(target.clone());
        }
    }
    schema.entity(seg).map(|e| e.name.clone())
}

// ── the bus ──────────────────────────────────────────────────────────────

/// A cross-node [`InvalidationBus`]: every subscriber on every replica
/// hears every replica's commits, because the signal travels over Postgres
/// rather than over one process's broadcast channel.
///
/// # One listener per database, held while a feed needs it
///
/// A `NOTIFY` reaches only sessions connected to the database it was sent
/// in, and a write notifies in its own transaction on the database its space
/// lives in. So once spaces can live in databases of their own
/// (`plan-space-routing.md` S7), one listener on the control database hears
/// nothing from an attached space, and that space's `?live` subscribers go
/// silently stale.
///
/// A `LISTEN` is a session, so it is one server connection for as long as it
/// is held, and Postgres offers no way to listen to two databases on one
/// connection. Held for every tenant's database at once, that is one
/// connection per tenant for good, which is what ran a default server out at
/// about twelve tenants (25g). So a routed bus listens **only while a feed is
/// open**: [`InvalidationBus::subscribe_in`] resolves the feed's space to its
/// database, opens a listener there if none is open, and counts the feed. When
/// the last feed on a database has ended the listener is closed after the idle
/// period (`ORDO_PG_IDLE_SECS`), so a page that reloads does not dial again.
/// A tenant nobody is watching costs no listening connection.
///
/// The deployment's own database is listened to for the bus's life, because
/// the spaces that have no database of their own live there.
///
/// The payload is still only an entity name: hearing one from any database
/// wakes every subscriber of that entity's ops, and each refetches under its
/// own scope, so listening more widely costs wake-ups and discloses nothing.
pub struct PgInvalidationBus {
    /// Per-op fan-out inside this process. The listeners publish into it;
    /// nothing else does.
    local: Arc<MemoryBus>,
    index: SharedIndex,
    metrics: Option<Arc<crate::metrics::Metrics>>,
    /// Where the databases come from, for a routed bus. `None` for a bus on
    /// one database, which listens to it for its life.
    pools: Option<Arc<dyn ordo_exec::route::PoolResolver>>,
    /// How long a database with no feed keeps its listener. `None` keeps it.
    linger: Option<std::time::Duration>,
    listening: Arc<Listening>,
    /// Serialises opening a session, so two feeds on one database cannot
    /// both open a listener for it.
    dial: tokio::sync::Mutex<()>,
}

/// Every `LISTEN` session this bus holds, by database.
#[derive(Default)]
struct Listening {
    /// Per database: whether it is heard now, and how many times it was
    /// lost. The public cache watches it (`design-public-path.md` §3, "A
    /// lost listener").
    hearing:
        std::sync::Mutex<std::collections::HashMap<String, tokio::sync::watch::Sender<Hearing>>>,
    held: std::sync::Mutex<std::collections::HashMap<String, Held>>,
    /// Databases whose listener could not be opened or has ended, one line
    /// each, by database.
    unheard: std::sync::Mutex<BTreeMap<String, String>>,
    next_session: std::sync::atomic::AtomicU64,
}

impl Listening {
    fn held(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, Held>> {
        self.held.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn unheard(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        self.unheard.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The hearing of one database, created on first ask.
    fn hearing_of(&self, key: &str) -> tokio::sync::watch::Receiver<Hearing> {
        let mut all = self.hearing.lock().unwrap_or_else(|p| p.into_inner());
        all.entry(key.to_string())
            .or_insert_with(|| tokio::sync::watch::channel(Hearing::default()).0)
            .subscribe()
    }

    /// Record that a database is heard, or that it was lost.
    fn set_heard(&self, key: &str, heard: bool) {
        let mut all = self.hearing.lock().unwrap_or_else(|p| p.into_inner());
        let tx = all
            .entry(key.to_string())
            .or_insert_with(|| tokio::sync::watch::channel(Hearing::default()).0);
        tx.send_modify(|h| {
            if h.heard && !heard {
                h.losses += 1;
            }
            h.heard = heard;
        });
    }
}

struct Held {
    cfg: PoolConfig,
    /// Feeds counting on this database.
    leases: usize,
    /// Never released: the deployment's own database, or the one database of
    /// a bus that does not route.
    permanent: bool,
    /// Bumped whenever the count changes, so a release timer that fires after
    /// a feed arrived can tell it has been overtaken.
    generation: u64,
    /// The open session and its number. Dropping it closes the connection.
    session: Option<(u64, ordo_exec::jobs::Listener)>,
}

/// A feed's claim on a database's listener, released by dropping it.
struct Lease {
    listening: Arc<Listening>,
    key: String,
    linger: Option<std::time::Duration>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let generation = {
            let mut held = self.listening.held();
            let Some(entry) = held.get_mut(&self.key) else {
                return;
            };
            entry.leases = entry.leases.saturating_sub(1);
            if entry.leases > 0 || entry.permanent {
                return;
            }
            entry.generation += 1;
            entry.generation
        };
        let Some(linger) = self.linger else { return };
        let (listening, key) = (self.listening.clone(), self.key.clone());
        let release = move || {
            let mut held = listening.held();
            if held
                .get(&key)
                .is_some_and(|e| e.leases == 0 && !e.permanent && e.generation == generation)
            {
                held.remove(&key);
                listening.unheard().remove(&key);
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) if !linger.is_zero() => {
                runtime.spawn(async move {
                    tokio::time::sleep(linger).await;
                    release();
                });
            }
            _ => release(),
        }
    }
}

/// Republish what `rx` hears into `publish`, one op at a time, and say
/// `ended` when the session goes away.
fn forward(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<ordo_exec::jobs::Notification>,
    index: SharedIndex,
    publish: Arc<MemoryBus>,
    metrics: Option<Arc<crate::metrics::Metrics>>,
    ended: impl FnOnce() + Send + 'static,
) {
    tokio::spawn(async move {
        // `recv` returning `None` means the listener's connection died, or
        // was closed on purpose. The task ends rather than spinning: the bus
        // has stopped being live and pretending otherwise is the failure 8p
        // describes.
        while let Some(n) = rx.recv().await {
            if n.channel != INVALIDATE_CHANNEL {
                continue;
            }
            if let Some(metrics) = &metrics {
                metrics.incr(
                    crate::metrics::INVALIDATIONS,
                    &[("source", "listener"), ("entity", &n.payload)],
                );
            }
            let current = index.read().map(|i| i.clone()).unwrap_or_default();
            for op in current.get(&n.payload).into_iter().flatten() {
                publish.publish(op);
            }
        }
        ended();
    });
}

/// How a database is named on this bus: the label every operator surface
/// uses.
fn key_of(cfg: &PoolConfig) -> String {
    ordo_exec::route::target_label(&cfg.host, cfg.port, &cfg.db)
}

impl PgInvalidationBus {
    fn new(
        f: &Fixture,
        pools: Option<Arc<dyn ordo_exec::route::PoolResolver>>,
        metrics: Option<Arc<crate::metrics::Metrics>>,
    ) -> PgInvalidationBus {
        PgInvalidationBus {
            local: Arc::new(MemoryBus::new()),
            index: Arc::new(std::sync::RwLock::new(Arc::new(entity_index(f)))),
            metrics,
            pools,
            linger: ordo_exec::PoolLimits::from_env().idle_timeout,
            listening: Arc::new(Listening::default()),
            dial: tokio::sync::Mutex::new(()),
        }
    }

    /// Hold a listener on `cfg`'s database for the bus's life.
    async fn hold_for_life(&self, cfg: PoolConfig) -> Result<(), ordo_exec::PoolError> {
        let key = key_of(&cfg);
        self.listening
            .held()
            .entry(key.clone())
            .or_insert_with(|| Held {
                cfg,
                leases: 0,
                permanent: false,
                generation: 0,
                session: None,
            })
            .permanent = true;
        self.ensure(&key).await.map(|_| ())
    }

    /// Open the session for `key` if it has none. Returns whether one was
    /// opened. A failure is remembered in [`PgInvalidationBus::unheard`] and
    /// returned.
    async fn ensure(&self, key: &str) -> Result<bool, ordo_exec::PoolError> {
        let _dialling = self.dial.lock().await;
        let cfg = match self.listening.held().get(key) {
            Some(e) if e.session.is_none() => e.cfg.clone(),
            _ => return Ok(false),
        };
        match ordo_exec::jobs::listen(&cfg, &[INVALIDATE_CHANNEL]).await {
            Ok((listener, rx)) => {
                let session = self
                    .listening
                    .next_session
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let (listening, name) = (self.listening.clone(), key.to_string());
                forward(
                    rx,
                    self.index.clone(),
                    self.local.clone(),
                    self.metrics.clone(),
                    move || {
                        // A session that ended without being released is a
                        // listener that died. Say so, so the next refresh
                        // or feed opens another.
                        let mut held = listening.held();
                        if let Some(e) = held.get_mut(&name) {
                            if e.session.as_ref().is_some_and(|(n, _)| *n == session) {
                                e.session = None;
                                listening.set_heard(&name, false);
                                listening
                                    .unheard()
                                    .insert(name.clone(), format!("{name}: the session ended"));
                            }
                        }
                    },
                );
                let mut held = self.listening.held();
                match held.get_mut(key) {
                    Some(e) => e.session = Some((session, listener)),
                    // Released while it was opening: the listener drops here.
                    None => return Ok(false),
                }
                self.listening.unheard().remove(key);
                drop(held);
                self.listening.set_heard(key, true);
                Ok(true)
            }
            Err(e) => {
                self.listening
                    .unheard()
                    .insert(key.to_string(), format!("{key}: {e}"));
                Err(e)
            }
        }
    }

    /// Open the listener and start republishing. `cfg` names the database
    /// the deployment writes to — the notification travels through it, so a
    /// listener pointed elsewhere hears nothing.
    ///
    /// `metrics`, when given, counts every republished signal, which is
    /// what makes "did node B ever hear from node A" answerable in
    /// production rather than only in a test.
    pub async fn connect(
        f: &Fixture,
        cfg: &PoolConfig,
        metrics: Option<Arc<crate::metrics::Metrics>>,
    ) -> Result<PgInvalidationBus, ordo_exec::PoolError> {
        let bus = PgInvalidationBus::new(f, None, metrics);
        bus.hold_for_life(cfg.clone()).await?;
        Ok(bus)
    }

    /// A bus that listens to the deployment's own database now, and to a
    /// tenant's database while a feed on it is open
    /// ([`InvalidationBus::subscribe_in`]).
    ///
    /// The deployment's own database failing to accept a listener refuses the
    /// boot, as it does for [`PgInvalidationBus::connect`]. A tenant's
    /// database that cannot be listened to does not stop the others: it is
    /// named in [`PgInvalidationBus::unheard`], and the next
    /// [`PgInvalidationBus::refresh`] tries it again.
    pub async fn connect_routed(
        f: &Fixture,
        pools: Arc<dyn ordo_exec::route::PoolResolver>,
        metrics: Option<Arc<crate::metrics::Metrics>>,
    ) -> Result<PgInvalidationBus, ordo_exec::PoolError> {
        let default = pools.default_pool().await?.config();
        let bus = PgInvalidationBus::new(f, Some(pools), metrics);
        bus.hold_for_life(default).await?;
        Ok(bus)
    }

    /// How long a database keeps its listener after its last feed ends.
    /// `None` keeps it. Defaults to `ORDO_PG_IDLE_SECS`.
    /// Install the entity index of a newly served version. Called on every
    /// deploy, so the ops it adds are invalidated by the writes they read.
    pub fn reindex(&self, f: &Fixture) {
        let next = Arc::new(entity_index(f));
        if let Ok(mut slot) = self.index.write() {
            *slot = next;
        }
    }

    /// Mark a database lost, as the listener's end does. For tests of what
    /// the public cache does when the bus stops hearing.
    pub fn lose_for_test(&self) {
        let keys: Vec<String> = self.listening.held().keys().cloned().collect();
        for k in keys {
            self.listening.set_heard(&k, false);
        }
    }

    /// Mark every database heard again. For tests, beside
    /// [`Self::lose_for_test`].
    pub fn hear_for_test(&self) {
        let keys: Vec<String> = self.listening.held().keys().cloned().collect();
        for k in keys {
            self.listening.set_heard(&k, true);
        }
    }

    pub fn with_linger(mut self, linger: Option<std::time::Duration>) -> Self {
        self.linger = linger;
        self
    }

    /// Open a listener on every database the bus should be listening to and
    /// is not: the deployment's own if it has none, and any a feed is open on
    /// whose session failed or ended. Returns how many were opened.
    ///
    /// A tenant's database is not opened here without a feed: that is the
    /// cost this bus exists to avoid. A node that has just provisioned a
    /// tenant, and a periodic tick on every other node, call this and pay
    /// for nothing until someone opens a feed. A bus on one database has
    /// nothing to refresh.
    pub async fn refresh(&self) -> Result<usize, ordo_exec::PoolError> {
        let Some(pools) = &self.pools else {
            return Ok(0);
        };
        let own = pools.default_pool().await?.config();
        self.hold_for_life(own).await.ok();
        let idle: Vec<String> = self
            .listening
            .held()
            .iter()
            .filter(|(_, e)| e.session.is_none())
            .map(|(k, _)| k.clone())
            .collect();
        let mut added = 0;
        for key in idle {
            if matches!(self.ensure(&key).await, Ok(true)) {
                added += 1;
            }
        }
        Ok(added)
    }

    /// Databases a listener could not be opened on, or that lost theirs.
    /// Feeds there hear nothing until a refresh reaches them, which is why
    /// this is reported rather than logged away.
    pub fn unheard(&self) -> Vec<String> {
        self.listening.unheard().values().cloned().collect()
    }

    /// How many `LISTEN` sessions this bus holds open now, which is how many
    /// server connections it costs.
    pub fn listeners(&self) -> usize {
        self.listening
            .held()
            .values()
            .filter(|e| e.session.is_some())
            .count()
    }

    /// Count a feed on `cfg`'s database and make sure it is listened to.
    async fn lease(&self, cfg: PoolConfig) -> Lease {
        let key = key_of(&cfg);
        {
            let mut held = self.listening.held();
            let entry = held.entry(key.clone()).or_insert_with(|| Held {
                cfg,
                leases: 0,
                permanent: false,
                generation: 0,
                session: None,
            });
            entry.leases += 1;
            entry.generation += 1;
        }
        // A failure is in `unheard`, and the feed opens regardless: its first
        // load does not depend on hearing anything.
        let _ = self.ensure(&key).await;
        Lease {
            listening: self.listening.clone(),
            key,
            linger: self.linger,
        }
    }
}

impl InvalidationBus for PgInvalidationBus {
    fn subscribe(&self, op_name: &str) -> broadcast::Receiver<InvalidationEvent> {
        self.local.subscribe(op_name)
    }

    fn entities_of(&self, op_name: &str) -> Vec<String> {
        let index = self.index.read().map(|i| i.clone()).unwrap_or_default();
        index
            .iter()
            .filter(|(_, ops)| ops.iter().any(|o| o == op_name))
            .map(|(e, _)| e.clone())
            .collect()
    }

    fn subscribe_in<'a>(&'a self, op_name: &'a str, space: &'a str) -> Subscribing<'a> {
        Box::pin(async move {
            // Subscribe first, so a signal between the listener opening and
            // the caller's first read is not lost.
            let receiver = self.local.subscribe(op_name);
            let mut hearing = None;
            let lease = match &self.pools {
                Some(pools) => match pools
                    .resolve(&ordo_crypto::SpaceId(space.to_string()))
                    .await
                {
                    Ok(pool) => {
                        let lease = self.lease(pool.config()).await;
                        hearing = Some(self.listening.hearing_of(&lease.key));
                        Some(Box::new(lease) as Box<dyn std::any::Any + Send + Sync>)
                    }
                    Err(e) => {
                        self.listening
                            .unheard()
                            .insert(format!("space {space}"), format!("space {space}: {e}"));
                        None
                    }
                },
                // One database, held for life: its hearing is the bus's.
                None => {
                    let key = self.listening.held().keys().next().cloned();
                    hearing = key.map(|k| self.listening.hearing_of(&k));
                    None
                }
            };
            Subscription {
                receiver,
                lease,
                hearing,
            }
        })
    }
}
