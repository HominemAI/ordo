//! 7b — the mandatory deny-volume alert, and what a deployment still has to
//! supply.
//!
//! `implementation.md` (Observability) marks **deny volume per principal per
//! space** mandatory: "a deployment that silences these is misconfigured and
//! the dashboard says so." The counter has existed since 8o
//! (`ordo_denials_total{space,principal,principal_kind,op}`); what did not
//! exist was anything that could *fire*. This is that: a rule with a
//! deployment-settable threshold, a sliding-window evaluation over the
//! counters, and a rendering an operator can read.
//!
//! # How it is evaluated
//!
//! Counters are monotone, so a rate needs two observations. [`DenyVolumeMonitor`]
//! keeps, per `(space, principal)`, a short series of `(instant, total)`
//! samples and fires when the **increase** across the trailing
//! [`DenyVolumeRule::window_secs`] reaches [`DenyVolumeRule::threshold`] —
//! the same quantity `increase(ordo_denials_total[5m])` computes in a
//! Prometheus rule, computed here because there is no Prometheus here.
//!
//! Three choices inside that, each deliberate:
//!
//! - **Aggregated over `op` and `principal_kind`.** The alert the doc marks
//!   mandatory is per principal per space, and a sweep is *one principal
//!   trying many ops*. Alerting per op would give each op its own small
//!   count and miss exactly the pattern that matters. The per-op series
//!   still exists in the registry for the operator who wants to see which
//!   ops were probed.
//! - **A new series baselines at zero.** The first sample for a
//!   `(space, principal)` pair is measured against zero rather than against
//!   itself, so a burst inside the very first window fires instead of
//!   silently establishing a baseline. A counter that appears at 500 *is*
//!   500 denials.
//! - **Firing is a level, not an edge.** [`DenyVolumeMonitor::evaluate`]
//!   returns everything currently over the threshold, so a firing clears by
//!   itself when the window rolls past the burst. Nothing here dedupes or
//!   routes notifications, because that is an alertmanager's job and there
//!   is not one.
//!
//! # What a deployment must still supply — the honest list
//!
//! This is one process's view of one process's counters. Specifically:
//!
//! 1. **A scraper or an exporter.** `Metrics::render_prometheus` renders;
//!    nothing pushes, and `/metrics` must be scraped by something. This
//!    monitor is a *second* evaluator that exists so a single-node
//!    deployment is not defenceless — it is not a replacement for the
//!    deployment's alerting stack.
//! 2. **Cross-replica aggregation.** Counters are per process and reset on
//!    restart. A principal spreading a sweep across four replicas produces
//!    four sub-threshold series and fires nothing here. Only a scraper
//!    summing by `(space, principal)` across instances sees it. This is the
//!    single most important limitation and it is not fixable in-process.
//! 3. **Routing.** Nothing pages anyone. [`DenyVolumeMonitor::render`] and
//!    the `/security/alerts` route are a surface an operator (or a probe)
//!    reads; delivery is theirs.
//! 4. **Persistence.** The sample series lives in memory. A restart forgets
//!    what was firing.
//! 5. **A threshold that fits the deployment.** [`DenyVolumeRule::from_env`]
//!    reads `ORDO_DENY_ALERT_THRESHOLD` and `ORDO_DENY_ALERT_WINDOW_SECS`.
//!    Unset does **not** disable the alert — a mandatory control that is off
//!    by default is not mandatory — it runs on the built-in default and
//!    reports [`DenyVolumeRule::configured`] as false, which the rendering
//!    says out loud so the dashboard can say the deployment is
//!    misconfigured.
//!
//! # The key-release breadth alert
//!
//! Not built here, and the reason is the same one `ordo-host`'s
//! `Metrics::key_release` states: `release(space, purpose, correlation)`
//! never sees a principal, so "distinct spaces per principal" needs the
//! structured request log to resolve a correlation to a principal. The
//! substrate this module evaluates over would serve it unchanged the day
//! that log exists.

use std::collections::BTreeMap;

use ordo_commandlog::{CorrelationId, SpaceId, Timestamp};

use crate::event::{EventKind, Scope, Subject};
use crate::store::SecurityStore;

/// The metric family this rule reads. Kept as a constant here rather than
/// imported from `ordo-host` so the rule stays testable in the leaf crate
/// and the host is the one that adapts, not the other way round.
pub const DENIALS_METRIC: &str = "ordo_denials_total";

/// Environment variable naming the deployment's threshold.
pub const THRESHOLD_ENV: &str = "ORDO_DENY_ALERT_THRESHOLD";
/// Environment variable naming the deployment's evaluation window.
pub const WINDOW_ENV: &str = "ORDO_DENY_ALERT_WINDOW_SECS";

/// The rule: how many denials from one principal in one space, over how
/// long, before this is an incident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DenyVolumeRule {
    pub threshold: u64,
    pub window_secs: u64,
    /// Did the deployment actually set this, or is it the built-in default?
    /// Surfaced rather than hidden: a mandatory alert running on a guess is
    /// a fact the operator needs to see.
    pub configured: bool,
}

impl DenyVolumeRule {
    /// Fifty denials from one principal in one space inside five minutes.
    ///
    /// A deliberately unambitious default. Ordinary use produces denials —
    /// a stale tab, a revoked grant, a client that retries — but it does not
    /// produce fifty from one principal in one space in five minutes, and
    /// every disclosure oracle in the security review is a high-call-count
    /// pattern that produces far more.
    pub const DEFAULT_THRESHOLD: u64 = 50;
    pub const DEFAULT_WINDOW_SECS: u64 = 300;

    pub fn new(threshold: u64, window_secs: u64) -> DenyVolumeRule {
        DenyVolumeRule {
            threshold: threshold.max(1),
            window_secs: window_secs.max(1),
            configured: true,
        }
    }

    /// The deployment's rule, or the built-in default marked as such.
    pub fn from_env() -> DenyVolumeRule {
        let threshold = std::env::var(THRESHOLD_ENV)
            .ok()
            .and_then(|v| v.parse().ok());
        let window = std::env::var(WINDOW_ENV).ok().and_then(|v| v.parse().ok());
        match (threshold, window) {
            (None, None) => DenyVolumeRule::default(),
            (t, w) => DenyVolumeRule::new(
                t.unwrap_or(DenyVolumeRule::DEFAULT_THRESHOLD),
                w.unwrap_or(DenyVolumeRule::DEFAULT_WINDOW_SECS),
            ),
        }
    }
}

impl Default for DenyVolumeRule {
    fn default() -> DenyVolumeRule {
        DenyVolumeRule {
            threshold: DenyVolumeRule::DEFAULT_THRESHOLD,
            window_secs: DenyVolumeRule::DEFAULT_WINDOW_SECS,
            configured: false,
        }
    }
}

/// One `(space, principal)` pair over the threshold right now.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Firing {
    pub space: String,
    pub principal: String,
    /// Denials counted across the trailing window.
    pub count: u64,
    pub threshold: u64,
    pub window_secs: u64,
    /// The instant of the oldest sample the count was measured from.
    pub since: Timestamp,
    pub at: Timestamp,
}

impl Firing {
    pub fn render(&self) -> String {
        format!(
            "FIRING deny-volume: principal `{}` in space `{}` — {} denials in {}s \
             (threshold {}), window [{}..{}]",
            self.principal,
            self.space,
            self.count,
            self.window_secs,
            self.threshold,
            self.since.0,
            self.at.0
        )
    }
}

/// One counter series as a registry hands it over: name, sorted labels,
/// value.
///
/// The monitor is fed these triples rather than a reference to
/// `ordo-host`'s registry, so the rule has no dependency on the process that
/// counts: any producer that can name a metric, its labels and its value can
/// be evaluated, including a scraper's federated view.
pub type CounterSeries = (String, Vec<(String, String)>, u64);

#[derive(Debug, Clone)]
struct Sample {
    at: Timestamp,
    total: u64,
}

/// The evaluator. One per process; fed a counter snapshot on a tick.
#[derive(Debug)]
pub struct DenyVolumeMonitor {
    rule: DenyVolumeRule,
    /// `(space, principal)` → trailing samples, oldest first.
    samples: BTreeMap<(String, String), Vec<Sample>>,
}

impl Default for DenyVolumeMonitor {
    fn default() -> DenyVolumeMonitor {
        DenyVolumeMonitor::new(DenyVolumeRule::default())
    }
}

impl DenyVolumeMonitor {
    pub fn new(rule: DenyVolumeRule) -> DenyVolumeMonitor {
        DenyVolumeMonitor {
            rule,
            samples: BTreeMap::new(),
        }
    }

    pub fn rule(&self) -> DenyVolumeRule {
        self.rule
    }

    /// Take one counter snapshot and report everything currently firing.
    ///
    /// `snapshot` is every series the registry holds; only
    /// [`DENIALS_METRIC`] is read, and it is aggregated by
    /// `(space, principal)` — summing over `op` and `principal_kind`,
    /// because the mandatory alert is per principal per space and a sweep
    /// spends itself across ops.
    pub fn evaluate(&mut self, snapshot: &[CounterSeries], now: Timestamp) -> Vec<Firing> {
        let mut totals: BTreeMap<(String, String), u64> = BTreeMap::new();
        for (name, labels, value) in snapshot {
            if name != DENIALS_METRIC {
                continue;
            }
            let get = |k: &str| {
                labels
                    .iter()
                    .find(|(lk, _)| lk == k)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            };
            *totals.entry((get("space"), get("principal"))).or_insert(0) += value;
        }

        // Series that vanished from the snapshot keep their history until it
        // ages out of the window; a series does not un-happen because the
        // registry dropped it.
        for (key, total) in &totals {
            let series = self.samples.entry(key.clone()).or_default();
            series.push(Sample {
                at: now,
                total: *total,
            });
        }
        let cutoff = now.0.saturating_sub(self.rule.window_secs);
        let mut firing = Vec::new();
        self.samples.retain(|key, series| {
            // Keep one sample from before the cutoff: it is the baseline the
            // increase is measured against. Anything older is redundant.
            let first_inside = series.iter().position(|s| s.at.0 >= cutoff).unwrap_or(0);
            let drop = first_inside.saturating_sub(1);
            series.drain(0..drop);
            let Some(last) = series.last() else {
                return false;
            };
            // A series whose newest sample is itself outside the window has
            // gone quiet; drop it rather than keep firing on stale data.
            if last.at.0 < cutoff {
                return false;
            }
            // The baseline: the newest sample at or before the cutoff, or
            // zero for a series that did not exist when the window opened.
            let (base_total, base_at) = match series.iter().find(|s| s.at.0 < cutoff) {
                Some(s) => (s.total, s.at),
                None => (0, series.first().expect("non-empty").at),
            };
            let count = last.total.saturating_sub(base_total);
            if count >= self.rule.threshold {
                firing.push(Firing {
                    space: key.0.clone(),
                    principal: key.1.clone(),
                    count,
                    threshold: self.rule.threshold,
                    window_secs: self.rule.window_secs,
                    since: base_at,
                    at: last.at,
                });
            }
            true
        });
        firing.sort();
        firing
    }

    /// The operator's view: the rule, whether the deployment set it, and
    /// every pair currently over it.
    pub fn render(&mut self, snapshot: &[CounterSeries], now: Timestamp) -> String {
        let firing = self.evaluate(snapshot, now);
        let mut out = String::new();
        out.push_str(&format!(
            "# deny-volume alert (mandatory: implementation.md, Observability)\n\
             # rule: {} denials per principal per space in {}s\n",
            self.rule.threshold, self.rule.window_secs
        ));
        if !self.rule.configured {
            out.push_str(&format!(
                "# MISCONFIGURED: {THRESHOLD_ENV}/{WINDOW_ENV} are unset — running on the \
                 built-in default\n"
            ));
        }
        out.push_str(
            "# scope: this process only. No cross-replica aggregation, no routing, no \
             persistence.\n",
        );
        if firing.is_empty() {
            out.push_str("ok: nothing over threshold\n");
        }
        for f in &firing {
            out.push_str(&f.render());
            out.push('\n');
        }
        out
    }
}

/// The correlation ids the security stream recorded for a firing — the
/// handles that turn "this principal is being denied a lot" into "these are
/// the requests".
///
/// This is the join the alert would otherwise not have. A counter has no
/// memory of individual requests; the stream does, in the bounded sample
/// every collapsed entry keeps. Reading it is a scan of the trailing entries
/// of one scope, which is why the caller passes a bound.
pub fn correlations_for(
    store: &dyn SecurityStore,
    firing: &Firing,
    scan_last: usize,
) -> Vec<CorrelationId> {
    let scope = if firing.space.is_empty() {
        Scope::Deployment
    } else {
        Scope::Space(SpaceId(firing.space.clone()))
    };
    let len = store.len(&scope);
    let from = len.saturating_sub(scan_last);
    let mut out = Vec::new();
    for e in store.entries(&scope, from, scan_last) {
        if !matches!(e.entry.kind, EventKind::Denied(_)) {
            continue;
        }
        let names = match &e.entry.subject {
            Subject::Attributed { actor, .. } => actor.to_string(),
            Subject::Unverified => "unverified".to_string(),
        };
        if !names.contains(&firing.principal) {
            continue;
        }
        if e.entry.last_at.0 < firing.since.0 {
            continue;
        }
        for c in &e.entry.correlations {
            if !out.contains(c) {
                out.push(c.clone());
            }
        }
    }
    out
}
