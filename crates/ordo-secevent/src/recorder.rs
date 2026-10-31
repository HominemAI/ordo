//! The routing rule, and the thing the door actually calls.
//!
//! # Members only — the load-bearing rule
//!
//! §6.1: "A denial appends to a space's stream **only when the denied
//! principal has a relationship with that space**." Everything else is
//! bookkeeping; this is the rule that decides whether the design is sound.
//! Four reasons, each fatal alone, and they are why the record is not more
//! rows in the command chain:
//!
//! 1. The space is bound *before* the access check — it has to be, the
//!    check needs it. So a stranger picks a victim space id and a plausible
//!    op name and, without the rule, grows that space's audit spine at will.
//! 2. A denial that took the command chain's per-space append lock would
//!    let a caller with no authority in a space contend with every
//!    legitimate writer in it. (This stream has its own lock; see
//!    `ordo_exec::secevent`.)
//! 3. A rate-collapsed count cannot be incremented inside an already-hashed
//!    entry — hence [`crate::collapse`].
//! 4. Entries cannot age out of a hash chain one at a time — hence whole
//!    prefixes ending at a checkpoint, in [`crate::store`].
//!
//! [`Relationship`] is the input the rule reads, and the door computes it
//! from the same state it just used to decide: a members row, or a live
//! capability grant scoped to this space.
//!
//! # Per space *or* deployment-wide — the answer is both, and routed
//!
//! The question "is a security event per space or deployment-wide" has no
//! single answer because the events do not all have a space. A denial names
//! one (the door bound it), but a denial *for a space the caller has nothing
//! to do with* has no space it may lawfully be written to, and an
//! unverifiable presentation has no principal to relate to any space at all.
//! So [`Scope`] is a routed property, not a configuration:
//!
//! - **`Scope::Space`** — the principal is a member of, or holds a live
//!   grant for, the space the door bound. The space owner reads this.
//! - **`Scope::Deployment`** — everything else: unverified callers,
//!   non-members, tokens naming an unrelated space, and the platform's own
//!   custody and key-release events, which belong to the operator rather
//!   than to any tenant.
//!
//! The consequence §6.1 names (a probe *sweeping* spaces is invisible in
//! any single space's stream) is answered by the deployment stream and by
//! `ordo_denials_total`, which is why the deny-volume alert
//! ([`crate::alert`]) is mandatory rather than nice to have. That answer
//! used to be incomplete in its own right: `route` decides *where* an
//! observation goes and used to be the only thing that saw the space it was
//! bound to, because [`Recorder::observe`] built the deployment entry from
//! the routed `scope` alone and the space fell on the floor between the two
//! (`plan-opslog.md` §0.2/§0.3). `observe` now carries that space through as
//! [`crate::event::StreamEntry::about`] whenever routing sends the
//! observation to the deployment, so the stream that exists precisely
//! because a sweep is invisible elsewhere can now say which spaces it swept.
//!
//! # Off the request path
//!
//! [`Recorder::observe`] does no I/O. It folds into an in-memory window or
//! queues a discrete entry, and a drainer appends. §6.1, pinned: a denial
//! must never delay a legitimate write, and an append failure is an
//! operational alert, never a failed request — the stream is evidence, not
//! a precondition. **Exactly one drainer** should own a recorder: two
//! drainers do not corrupt anything, they just split the entries between
//! them.
//!
//! # Bounded, and the argument for which bound
//!
//! The outbox has a hard cap ([`Recorder::DEFAULT_OUTBOX`]). It has to: a
//! process whose store is unreachable would otherwise grow a `Vec` for as
//! long as the outage lasts, and "the audit subsystem was the OOM" is a
//! worse outcome than any single lost denial. The three candidate
//! behaviours at the cap, and why this one:
//!
//! - **Refuse the request.** Rejected, and not narrowly. §6.1 already pins
//!   the opposite — "a stream append failure is an operational alert, never
//!   a failed request … the stream is evidence, not a precondition" — and
//!   the mechanics agree: `observe` is called *after* the door has decided,
//!   so there is nothing left to refuse. Making it refusable would mean
//!   moving the append onto the request path, which reintroduces every one
//!   of §6.1's four reasons at once, and it hands an attacker a denial of
//!   service that costs them one unreachable database.
//! - **Spill to local disk.** A second durable store, with its own
//!   retention, its own confidentiality (principal ids at rest outside the
//!   database's protection), its own replay-ordering problem when it is
//!   folded back into a *hash chain*, and its own failure mode on a pod
//!   with an ephemeral filesystem. It converts one bounded loss into an
//!   unbounded surface, and on Kubernetes the disk it spills to usually
//!   dies with the process anyway.
//! - **Drop, bounded, and declare the drop inside the record.** Taken.
//!
//! Two details make the third defensible rather than merely cheap:
//!
//! 1. **The oldest entries survive; arrivals are what is refused.** During
//!    a storm the onset is the evidence — who started, from where, against
//!    which op — and the ten-thousandth near-identical collapsed window is
//!    not. This is the same rule the per-window cap already applies, so the
//!    record degrades one way rather than two.
//! 2. **The loss becomes a hashed link.**
//!    [`EventKind::EventsDropped`] is appended to the scope's own stream as
//!    soon as the store is reachable again. A counter in a process that is
//!    about to restart is not visibility; a gap declared inside the
//!    artifact a reviewer is auditing is. It is the record saying *"between
//!    this entry and the next, N events happened that I cannot show you"* —
//!    which is exactly what an auditor needs and exactly what silent
//!    dropping denies them.
//!
//! Nothing here samples denials. Every observation is either recorded,
//! collapsed into an entry that carries its count, or counted into a
//! declared loss. There is no fourth path where an event quietly does not
//! happen.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use ordo_commandlog::{CorrelationId, SpaceId, Timestamp};
use ordo_identity::{AccessEngine, TokenClaims};

use crate::collapse::{CollapseKey, Collapser};
use crate::event::{
    sanitize_op, sanitize_space, EventKind, RecordedDecision, Scope, StreamEntry, Subject,
};
use crate::store::{SecurityStore, StreamError};

/// What the principal has to do with the space the door bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relationship {
    /// A members row in this space — suspended counts, since a suspended
    /// member is still someone the space knows.
    Member,
    /// A live capability grant scoped to this space.
    Grant,
    /// The platform holds this space's keys, and the event is a custody act
    /// on them. Owner-ruled 2026-09-14 (`key-release-home`): a tenant auditing
    /// their own custody is the party who should see that the platform
    /// opened their key, so custody acts about a space are recorded in that
    /// space's stream. The rule that keeps other parties out of a tenant's
    /// stream exists to stop a stranger writing into it, and the custodian is
    /// not a stranger. Passed only by the key provider's own wrapper.
    Custodian,
    /// Nothing. The event is the deployment's.
    None,
}

impl Relationship {
    pub fn exists(self) -> bool {
        !matches!(self, Relationship::None)
    }
}

/// Read the relationship out of the engine the door just consulted.
///
/// A membership row is a relationship whatever its status: `PrincipalSuspended`
/// is precisely the denial a space owner most needs to see in their own
/// stream, and routing it to the deployment because the member is stopped
/// would hide it from the only person who can act on it.
///
/// A capability grant counts only when it is scoped to *this* space — which
/// is the same comparison the door makes first, and for the same isolating
/// reason. A grant for another space is no relationship with this one.
pub fn relationship(engine: &AccessEngine, claims: &TokenClaims, space: &SpaceId) -> Relationship {
    let ispace = ordo_identity::SpaceId(space.0.clone());
    if engine
        .membership_status(&ispace, &claims.principal)
        .is_some()
    {
        return Relationship::Member;
    }
    match &claims.capability {
        Some(grant) if *grant.space() == ispace => Relationship::Grant,
        _ => Relationship::None,
    }
}

/// When something happened, and under which correlation.
///
/// The two travel together in one value rather than as two parameters
/// because they answer one question — *which unit of work was this* — and
/// because a call site that can transpose them is a call site that will.
#[derive(Debug, Clone)]
pub struct Stamp {
    pub at: Timestamp,
    pub correlation: CorrelationId,
}

impl Stamp {
    pub fn new(at: Timestamp, correlation: CorrelationId) -> Stamp {
        Stamp { at, correlation }
    }
}

/// One thing that happened, before routing and before collapse.
#[derive(Debug, Clone)]
pub struct Observation {
    /// The space the door bound, if the request named one. `None` for
    /// events that are deployment-wide by nature.
    ///
    /// `route` reads this to decide the destination; when the destination
    /// turns out to be [`Scope::Deployment`], [`Recorder::observe`] also
    /// carries it into the entry as [`crate::event::StreamEntry::about`];
    /// the space is then the *object* of the event rather than its home,
    /// and this is the one value both facts are read from. Caller-supplied,
    /// admitted through [`crate::event::sanitize_space`] on the way in.
    pub space: Option<SpaceId>,
    pub subject: Subject,
    /// Raw, as presented. [`sanitize_op`] runs on the way in.
    pub op: String,
    pub kind: EventKind,
    pub at: Timestamp,
    /// The id that joins this event to the command group, the key lease,
    /// and (when it exists) the request trace. See [`crate::correlation`].
    pub correlation: CorrelationId,
}

/// Route one observation. The whole of §6.1's members-only rule.
pub fn route(space: Option<&SpaceId>, subject: &Subject, rel: Relationship) -> Scope {
    match (space, subject) {
        // Nothing was proven, so no relationship can exist: an unverified
        // caller cannot be a member of anything.
        (_, Subject::Unverified) => Scope::Deployment,
        (Some(space), _) if rel.exists() => Scope::Space(space.clone()),
        _ => Scope::Deployment,
    }
}

/// The bounded outbox: entries waiting for a drainer, plus the per-scope
/// tally of what the bound refused.
#[derive(Debug)]
struct Outbox {
    /// Entries ready to append, oldest first. A `VecDeque` because a
    /// drainer that could not commit hands its batch back to the *front*,
    /// and re-prepending to a `Vec` is the one operation a `Vec` is bad at.
    queue: VecDeque<StreamEntry>,
    cap: usize,
    /// Per scope: observations the cap refused since the last time a
    /// marker for that scope was drained. Held outside [`Outbox::queue`]
    /// on purpose — a marker generated from a counter can never itself be
    /// the thing the cap discards.
    lost: BTreeMap<Scope, u64>,
}

impl Outbox {
    /// Returns how many observations the bound refused (0 or the entry's
    /// own count).
    fn push(&mut self, entry: StreamEntry) -> u64 {
        if self.queue.len() >= self.cap {
            let lost = entry.count.max(1);
            *self.lost.entry(entry.scope).or_insert(0) += lost;
            return lost;
        }
        self.queue.push_back(entry);
        0
    }

    /// Hand a batch back to the front, in order. What no longer fits is
    /// counted, not silently forgotten — and a marker never counts as its
    /// own loss: its `lost` folds back into the tally it came from, so the
    /// next drain regenerates it.
    fn requeue(&mut self, entries: Vec<StreamEntry>) -> u64 {
        let mut refused = 0u64;
        for entry in entries.into_iter().rev() {
            if let EventKind::EventsDropped { lost } = entry.kind {
                *self.lost.entry(entry.scope).or_insert(0) += lost;
                continue;
            }
            if self.queue.len() >= self.cap && !entry.kind.is_marker() {
                let lost = entry.count.max(1);
                *self.lost.entry(entry.scope).or_insert(0) += lost;
                refused += lost;
                continue;
            }
            self.queue.push_front(entry);
        }
        refused
    }

    /// One `EventsDropped` marker per scope with a non-zero tally, and the
    /// tallies cleared. Stamped at `now` because that is the only instant
    /// the recorder can honestly claim: the events it is declaring are
    /// gone, so their own instants went with them.
    fn take_drop_markers(&mut self, now: Timestamp) -> Vec<StreamEntry> {
        std::mem::take(&mut self.lost)
            .into_iter()
            .filter(|(_, lost)| *lost > 0)
            .map(|(scope, lost)| StreamEntry {
                scope,
                // No target: the loss it declares may span many spaces'
                // worth of refused entries, so no single `about` would be
                // honest, the same reasoning the cap's own marker follows.
                about: None,
                // The platform saying it stopped writing — the same
                // attribution the per-window cap's marker carries, because
                // it is the same kind of statement about the record.
                subject: Subject::platform("secevent"),
                op: String::new(),
                kind: EventKind::EventsDropped { lost },
                first_at: now,
                last_at: now,
                count: lost,
                correlations: Vec::new(),
            })
            .collect()
    }
}

/// The in-process recorder: routing, collapse, and a bounded queue for the
/// drainer.
#[derive(Debug)]
pub struct Recorder {
    collapser: Mutex<Collapser>,
    /// Entries ready to append, and the tally of what the bound refused.
    pending: Mutex<Outbox>,
    /// Lifetime totals, for a health surface that must not take the lock.
    observed: AtomicU64,
    dropped: AtomicU64,
}

impl Default for Recorder {
    fn default() -> Recorder {
        Recorder::new(Collapser::default())
    }
}

impl Recorder {
    /// How many entries the outbox holds before it starts declaring loss.
    ///
    /// Sized against the thing that fills it: the collapser emits at most
    /// [`crate::DEFAULT_CAP`] (256) entries per scope per window, so this
    /// is roughly sixty-four scope-windows of backlog — an outage of tens
    /// of minutes at the default one-minute window, for a deployment
    /// serving a handful of spaces, before anything is lost at all. At
    /// ~200 bytes an entry it is a few megabytes, which is a bound worth
    /// having and not a budget worth arguing over.
    pub const DEFAULT_OUTBOX: usize = 16_384;

    pub fn new(collapser: Collapser) -> Recorder {
        Recorder::with_outbox(collapser, Recorder::DEFAULT_OUTBOX)
    }

    /// An explicit outbox bound — for a deployment that has measured its
    /// own, and for the tests that have to reach the bound in a second.
    pub fn with_outbox(collapser: Collapser, cap: usize) -> Recorder {
        Recorder {
            collapser: Mutex::new(collapser),
            pending: Mutex::new(Outbox {
                queue: VecDeque::new(),
                cap: cap.max(1),
                lost: BTreeMap::new(),
            }),
            observed: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    /// Record one observation. Never blocks on I/O, never fails a request.
    pub fn observe(&self, obs: Observation, rel: Relationship) {
        let scope = route(obs.space.as_ref(), &obs.subject, rel);
        // The observation named a space either way: the door bound one, or
        // a key release names the one it touched. `route` already decided
        // whether that space is this entry's *home* or merely its *object*:
        // a space-scope entry already IS the space it names, so `about`
        // only ever carries a value when the destination is the deployment
        // stream. Sanitized here, at the one place every producer's space
        // passes through, for the same reason `sanitize_op` runs here and
        // not at each call site.
        let about = if scope.is_deployment() {
            obs.space.as_ref().map(|s| SpaceId(sanitize_space(&s.0)))
        } else {
            None
        };
        match obs.kind {
            EventKind::Denied(decisions) => {
                let key = CollapseKey {
                    scope,
                    about,
                    subject: obs.subject,
                    op: sanitize_op(&obs.op),
                };
                let mut c = self.collapser.lock().expect("collapser");
                for d in decisions {
                    c.observe(key.clone(), d, obs.at, obs.correlation.clone());
                }
            }
            kind => {
                let mut entry =
                    StreamEntry::single(scope, obs.subject, &obs.op, kind, obs.at, obs.correlation);
                entry.about = about;
                let lost = self.pending.lock().expect("pending").push(entry);
                if lost > 0 {
                    self.dropped.fetch_add(lost, Ordering::Relaxed);
                }
            }
        }
        self.observed.fetch_add(1, Ordering::Relaxed);
    }

    /// Convenience for the door: one denial, decided.
    pub fn deny(
        &self,
        space: Option<&SpaceId>,
        subject: Subject,
        op: &str,
        decision: RecordedDecision,
        stamp: &Stamp,
        rel: Relationship,
    ) {
        self.observe(
            Observation {
                space: space.cloned(),
                subject,
                op: op.to_string(),
                kind: EventKind::denied(decision),
                at: stamp.at,
                correlation: stamp.correlation.clone(),
            },
            rel,
        );
    }

    /// Entries whose windows have elapsed, plus every queued discrete
    /// entry. This is what the job queue drains.
    ///
    /// The drop markers come **first**, because they are a statement about
    /// the entries that would have preceded the ones behind them: a reader
    /// walking the stream meets *"N events are missing"* and then the
    /// entries that survived, in the order the outage produced them.
    ///
    /// Entries *leave* the recorder here. A caller whose append does not
    /// commit must hand them back with [`Recorder::requeue`]; anything
    /// else loses them, which is the bug this signature exists to make
    /// obvious.
    #[must_use = "drained entries are no longer in the recorder — append them or requeue them"]
    pub fn drain_ready(&self, now: Timestamp) -> Vec<StreamEntry> {
        let mut pending = self.pending.lock().expect("pending");
        let mut out = pending.take_drop_markers(now);
        out.extend(pending.queue.drain(..));
        drop(pending);
        out.extend(self.collapser.lock().expect("collapser").close_due(now));
        out
    }

    /// Everything, elapsed or not — shutdown, or an operator drain.
    #[must_use = "drained entries are no longer in the recorder — append them or requeue them"]
    pub fn drain_all(&self, now: Timestamp) -> Vec<StreamEntry> {
        let mut pending = self.pending.lock().expect("pending");
        let mut out = pending.take_drop_markers(now);
        out.extend(pending.queue.drain(..));
        drop(pending);
        out.extend(self.collapser.lock().expect("collapser").close_all());
        out
    }

    /// Give back entries whose append did not commit.
    ///
    /// They go to the front, in order, so the stream stays chronological
    /// across a retry. What no longer fits the bound is counted into the
    /// scope's loss tally and will be declared by the next
    /// [`EventKind::EventsDropped`] marker — never simply forgotten.
    pub fn requeue(&self, entries: Vec<StreamEntry>) {
        let refused = self.pending.lock().expect("pending").requeue(entries);
        if refused > 0 {
            self.dropped.fetch_add(refused, Ordering::Relaxed);
        }
    }

    /// Drain and append. The append is the only fallible part, and a
    /// failure here is an operational alert: the caller has already been
    /// answered.
    ///
    /// On failure the entries **from the failing one onward** go back into
    /// the outbox. Both of this workspace's stores append one entry per
    /// transaction, so the ones before it are committed and the failing one
    /// is not — requeuing exactly the suffix is the loss-free and
    /// duplicate-free split. Where the two cannot both be had (a commit
    /// whose acknowledgement was lost), this errs toward the duplicate: a
    /// repeated link is visible, chains correctly, and can be read past;
    /// a missing one is a silent hole in evidence.
    pub fn flush_into(
        &self,
        store: &mut dyn SecurityStore,
        now: Timestamp,
    ) -> Result<usize, StreamError> {
        let entries = self.drain_ready(now);
        let n = entries.len();
        for (i, e) in entries.iter().enumerate() {
            if let Err(err) = store.append(e.clone()) {
                self.requeue(entries[i..].to_vec());
                return Err(err);
            }
        }
        Ok(n)
    }

    /// Operator-facing prose, drained.
    ///
    /// This is what replaces the host's old `Mutex<Vec<String>>` audit
    /// buffer. The strings look the same; the difference is that they are
    /// now *rendered from* records that also go to a hash-chained stream,
    /// rather than being the only trace a denial left.
    pub fn take_rendered(&self, now: Timestamp) -> Vec<String> {
        self.drain_all(now)
            .iter()
            .map(StreamEntry::render)
            .collect()
    }

    /// How many windows are open — operational state, exposed for health
    /// surfaces and tests.
    pub fn open_windows(&self) -> usize {
        self.collapser.lock().expect("collapser").open_windows()
    }

    /// Entries waiting for a drainer. A number that climbs and does not
    /// come back down is a drainer that has stopped, and it is the
    /// earliest signal there is one.
    pub fn backlog(&self) -> usize {
        self.pending.lock().expect("pending").queue.len()
    }

    /// Observations the bound has refused over this process's life. The
    /// same number the [`EventKind::EventsDropped`] markers carry, exposed
    /// here so a health surface can report it without draining anything.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Observations handed to [`Recorder::observe`] over this process's
    /// life. With [`Recorder::dropped`] it is the denominator that makes
    /// the loss legible as a fraction rather than as a bare count.
    pub fn observed(&self) -> u64 {
        self.observed.load(Ordering::Relaxed)
    }

    /// Loss the outbox has tallied and not yet declared in the stream.
    /// Non-zero only between a drop and the next successful drain.
    pub fn undeclared_loss(&self) -> u64 {
        self.pending
            .lock()
            .expect("pending")
            .lost
            .values()
            .sum::<u64>()
    }
}
