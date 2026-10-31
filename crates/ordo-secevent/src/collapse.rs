//! Rate collapse: the open window is ordinary mutable state, and only its
//! closed summary is ever hashed.
//!
//! §6.1 gives the rule and the reason together. "Rate-collapsed with a
//! mutable count" is not expressible in an append-only structure — you
//! cannot increment a counter inside an entry that has already been hashed
//! — so nothing is appended while a window is open. The accumulator here is
//! a plain map; at close it becomes a [`StreamEntry`] with the count
//! *inside* the body, and from that moment it is immutable like every other
//! link.
//!
//! # The key is `(scope, about, subject, op)`
//!
//! §6.1 pins `(space, principal, op)`. For `Scope::Space` the space *is*
//! the scope, so `scope` alone carries it and `about` is always `None`;
//! this is the same key the doc names, unchanged. For `Scope::Deployment`
//! every observation shares one `scope`, so the doc's `space` has to come
//! from somewhere else: `about`, the target `Recorder::observe` (§1.2)
//! carried through from the routed observation. Without it (the bug this
//! module carried until `plan-opslog.md` D1b) a principal firing one op at
//! ten thousand spaces degenerated to exactly the `(principal, op)` key the
//! doc rejected: one open window, one closed entry, `count: 10000`,
//! indistinguishable from ten thousand retries against one space. With
//! `about` in the key, the same sweep opens ten thousand windows and closes
//! ten thousand entries, the property the doc's key exists to guarantee,
//! restored for the stream where it was silently missing.
//!
//! **The per-scope window cap ([`Collapser::open_per_scope`]) stays keyed on
//! `Scope` alone, deliberately not on `(Scope, about)`.** If the cap counted
//! per target, a sweep across ten thousand caller-chosen space ids would buy
//! ten thousand independent [`DEFAULT_CAP`]-entry budgets, and the attacker,
//! who chooses the space ids, would be the one sizing the record. Keeping
//! the cap on the scope means one sweep produces at most `DEFAULT_CAP`
//! distinct entries per window plus one [`crate::event::EventKind::Suppressed`]
//! marker, whatever variety of targets it tries: the record degrades before
//! it grows, and the shape of the degradation still says "many distinct
//! targets, and more were suppressed" rather than hiding the sweep behind a
//! budget the sweep itself controls.
//!
//! # What a closed entry carries that the doc's does not
//!
//! The doc says an entry carries the decision. A window keyed on
//! `(scope, subject, op)` can see more than one — `NoMembership` then
//! `PrincipalSuspended` after an operator acts, say — so the closed entry
//! carries the **set** of decisions observed, sorted, deduplicated. Adding
//! the decision to the collapse key instead would have been the other
//! option and is worse: it multiplies entries by a dimension the doc
//! deliberately left out of the key, and it is exactly the dimension an
//! attacker can vary. The set keeps every distinction §6.1 demands while
//! the key stays the one the doc pinned.

use std::collections::{BTreeMap, BTreeSet};

use ordo_commandlog::{CorrelationId, SpaceId, Timestamp};

use crate::event::{EventKind, RecordedDecision, Scope, StreamEntry, Subject, CORRELATION_SAMPLE};

/// The collapse key: `(scope, about, subject, op)`. See the module doc for
/// why `about` has to be a separate field from `scope` and why the
/// per-window cap must not read it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CollapseKey {
    pub scope: Scope,
    /// The target a deployment-scope observation named. `None` for a
    /// space-scope key and for a deployment observation with no target of
    /// its own (a key release names one; the cap's own `Suppressed` marker
    /// does not). Field order matches the doc's `(space, …)`: this sits
    /// right after `scope`, the field it stands in for on the deployment
    /// stream.
    pub about: Option<SpaceId>,
    pub subject: Subject,
    pub op: String,
}

#[derive(Debug, Clone)]
struct Window {
    decisions: BTreeSet<RecordedDecision>,
    first_at: Timestamp,
    last_at: Timestamp,
    count: u64,
    correlations: Vec<CorrelationId>,
}

/// The default collapse window: sixty seconds.
///
/// "A short window" in the doc's words. Short enough that a burst is still
/// legible as a burst when the entry lands, long enough that a determined
/// prober cannot make one entry per request by pacing themselves.
pub const DEFAULT_WINDOW_SECS: u64 = 60;

/// The default per-scope per-window entry cap.
///
/// A cap on distinct `(subject, op)` pairs, not on observations: a single
/// principal hammering one op is one entry however many times they try, and
/// the cap only bites on *variety*, which is the thing that would otherwise
/// grow the record linearly with an attacker's imagination.
pub const DEFAULT_CAP: usize = 256;

/// The accumulator. One per process; the platform drains it from the job
/// queue, off the request path.
#[derive(Debug)]
pub struct Collapser {
    window_secs: u64,
    cap: usize,
    open: BTreeMap<CollapseKey, Window>,
    /// How many windows are open per scope.
    ///
    /// Derivable from [`Collapser::open`] and kept anyway, because deriving
    /// it is what the cap check used to do: a `keys().filter(scope)` scan
    /// on **every observation that opens a new window**, holding the
    /// recorder's mutex, costing O(open windows). That is fastest when
    /// nothing is happening and slowest under a denial storm — the precise
    /// inversion you do not want on a request path. This map makes the cap
    /// check O(log scopes) and the cost flat in the size of the burst.
    open_per_scope: BTreeMap<Scope, usize>,
    /// Per scope: when the current window opened, and how many observations
    /// the cap refused since it did.
    window_start: BTreeMap<Scope, Timestamp>,
    suppressed: BTreeMap<Scope, u64>,
}

impl Default for Collapser {
    fn default() -> Collapser {
        Collapser::new(DEFAULT_WINDOW_SECS, DEFAULT_CAP)
    }
}

impl Collapser {
    pub fn new(window_secs: u64, cap: usize) -> Collapser {
        Collapser {
            window_secs,
            cap,
            open: BTreeMap::new(),
            open_per_scope: BTreeMap::new(),
            window_start: BTreeMap::new(),
            suppressed: BTreeMap::new(),
        }
    }

    pub fn window_secs(&self) -> u64 {
        self.window_secs
    }

    /// How many windows are open right now. Operational state, not record.
    pub fn open_windows(&self) -> usize {
        self.open.len()
    }

    /// Fold one denial into its window.
    pub fn observe(
        &mut self,
        key: CollapseKey,
        decision: RecordedDecision,
        at: Timestamp,
        correlation: CorrelationId,
    ) {
        self.window_start.entry(key.scope.clone()).or_insert(at);
        if let Some(w) = self.open.get_mut(&key) {
            w.decisions.insert(decision);
            w.last_at = at;
            w.count += 1;
            // A bounded sample of distinct correlations: enough to reach the
            // requests behind a burst, never enough to grow with it.
            if w.correlations.len() < CORRELATION_SAMPLE && !w.correlations.contains(&correlation) {
                w.correlations.push(correlation);
            }
            return;
        }
        if self.open_per_scope.get(&key.scope).copied().unwrap_or(0) >= self.cap {
            // The record degrades before it grows.
            *self.suppressed.entry(key.scope).or_insert(0) += 1;
            return;
        }
        *self.open_per_scope.entry(key.scope.clone()).or_insert(0) += 1;
        self.open.insert(
            key,
            Window {
                decisions: BTreeSet::from([decision]),
                first_at: at,
                last_at: at,
                count: 1,
                correlations: vec![correlation],
            },
        );
    }

    /// Close every scope whose window has elapsed, and hand back the
    /// entries to append. Nothing was hashed before this moment.
    pub fn close_due(&mut self, now: Timestamp) -> Vec<StreamEntry> {
        let due: Vec<Scope> = self
            .window_start
            .iter()
            .filter(|(_, start)| now.0.saturating_sub(start.0) >= self.window_secs)
            .map(|(s, _)| s.clone())
            .collect();
        self.close_scopes(&due)
    }

    /// Close everything, elapsed or not — shutdown, or a drain the operator
    /// asked for.
    pub fn close_all(&mut self) -> Vec<StreamEntry> {
        let all: Vec<Scope> = self.window_start.keys().cloned().collect();
        self.close_scopes(&all)
    }

    fn close_scopes(&mut self, scopes: &[Scope]) -> Vec<StreamEntry> {
        let mut out = Vec::new();
        for scope in scopes {
            let keys: Vec<CollapseKey> = self
                .open
                .keys()
                .filter(|k| k.scope == *scope)
                .cloned()
                .collect();
            for key in keys {
                let w = self.open.remove(&key).expect("just listed");
                out.push(StreamEntry {
                    scope: key.scope,
                    about: key.about,
                    subject: key.subject,
                    op: key.op,
                    kind: EventKind::Denied(w.decisions),
                    first_at: w.first_at,
                    last_at: w.last_at,
                    count: w.count,
                    correlations: w.correlations,
                });
            }
            self.open_per_scope.remove(scope);
            if let Some(further) = self.suppressed.remove(scope) {
                if further > 0 {
                    let at = self
                        .window_start
                        .get(scope)
                        .copied()
                        .unwrap_or(Timestamp(0));
                    out.push(StreamEntry {
                        scope: scope.clone(),
                        // The marker names no target: it aggregates every
                        // key the cap refused in this scope's window, which
                        // may span many distinct spaces, so no single
                        // `about` would be honest.
                        about: None,
                        // The marker is about the record, not about anyone:
                        // it is the platform saying it stopped writing, so
                        // it is attributed to the platform.
                        subject: Subject::platform("secevent"),
                        op: String::new(),
                        kind: EventKind::Suppressed { further },
                        first_at: at,
                        last_at: at,
                        count: further,
                        correlations: Vec::new(),
                    });
                }
            }
            self.window_start.remove(scope);
        }
        out
    }
}
