//! Setting up a person's own tenant, kept going until it is made (owner
//! rulings, 2026-10-21).
//!
//! # What the owner ruled
//!
//! - A tenant write before setup is ready is refused with a retryable
//!   `tenant_not_ready` ([`NotReady`], read by the door).
//! - Any running server keeps retrying a stalled setup, with no time limit
//!   ("it should never really stop trying"). Two servers never run one setup
//!   at once: the registry row carries a lease ([`crate::tenants::claim_setup`]).
//! - After `ORDO_PROVISION_STUCK_SECS` a setup is reported stuck: in
//!   `GET /auth/provisioning` for the person, and on the security-event
//!   stream, in `/health/deep` and as a metric for the app developer and the
//!   deployment owner.
//! - If a tenant can never be made, the account stays. Nothing is deleted.
//!
//! # The settings
//!
//! Every interval and cap here is a deployment setting with a default
//! ([`SetupSettings`]). A value outside its range refuses the boot and names
//! the setting, the range and the default.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::supervisor::{BoxFut, Cadence, Progress, Worker, WorkerCtx, WorkerError};
use crate::tenant_ops::TenantOps;

/// How often each server sweeps for stalled setups, in seconds.
pub const SWEEP_ENV: &str = "ORDO_PROVISION_SWEEP_SECS";
/// How long one server holds the right to run a setup, in seconds. It is
/// renewed while the setup runs, so it bounds only how long a server that
/// stopped keeps others waiting.
pub const LEASE_ENV: &str = "ORDO_PROVISION_LEASE_SECS";
/// The wait after the first failed try, in seconds. Each later wait doubles.
pub const RETRY_BASE_ENV: &str = "ORDO_PROVISION_RETRY_BASE_SECS";
/// The longest wait between tries, in seconds.
pub const RETRY_CAP_ENV: &str = "ORDO_PROVISION_RETRY_CAP_SECS";
/// How long a setup may take before it is reported stuck, in seconds.
pub const STUCK_ENV: &str = "ORDO_PROVISION_STUCK_SECS";
/// The `Retry-After` a `tenant_not_ready` refusal carries, in seconds.
pub const NOT_READY_RETRY_ENV: &str = "ORDO_PROVISION_NOT_READY_RETRY_SECS";

/// The metric: own tenants being set up now, deployment-wide.
pub const IN_PROGRESS_METRIC: &str = "ordo_tenant_setup_in_progress";
/// The metric: own tenants reported stuck now, deployment-wide.
pub const STUCK_METRIC: &str = "ordo_tenant_setup_stuck";

/// The intervals and caps of setting up an own tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetupSettings {
    pub sweep_secs: u64,
    pub lease_secs: u64,
    pub retry_base_secs: u64,
    pub retry_cap_secs: u64,
    pub stuck_secs: u64,
    pub not_ready_retry_secs: u64,
}

impl Default for SetupSettings {
    fn default() -> SetupSettings {
        SetupSettings {
            sweep_secs: 30,
            lease_secs: 120,
            retry_base_secs: 15,
            retry_cap_secs: 900,
            stuck_secs: 300,
            not_ready_retry_secs: 2,
        }
    }
}

impl SetupSettings {
    /// Read from the environment. `Err` names the setting and why.
    pub fn from_env() -> Result<SetupSettings, (&'static str, String)> {
        SetupSettings::from_vars(&|k: &str| std::env::var(k).ok())
    }

    /// Read through `get`, for a test that does not touch the process
    /// environment.
    pub fn from_vars(
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<SetupSettings, (&'static str, String)> {
        let d = SetupSettings::default();
        let read = |var: &'static str, default: u64, min: u64, max: u64| {
            let Some(raw) = get(var) else {
                return Ok(default);
            };
            match raw.trim().parse::<u64>() {
                Ok(v) if (min..=max).contains(&v) => Ok(v),
                _ => Err((
                    var,
                    format!(
                        "`{raw}` is not a whole number of seconds from {min} to {max}. \
                         Unset it for the default, {default}"
                    ),
                )),
            }
        };
        let s = SetupSettings {
            sweep_secs: read(SWEEP_ENV, d.sweep_secs, 1, 3_600)?,
            lease_secs: read(LEASE_ENV, d.lease_secs, 10, 3_600)?,
            retry_base_secs: read(RETRY_BASE_ENV, d.retry_base_secs, 1, 3_600)?,
            retry_cap_secs: read(RETRY_CAP_ENV, d.retry_cap_secs, 1, 86_400)?,
            stuck_secs: read(STUCK_ENV, d.stuck_secs, 1, 86_400)?,
            not_ready_retry_secs: read(NOT_READY_RETRY_ENV, d.not_ready_retry_secs, 1, 300)?,
        };
        if s.retry_cap_secs < s.retry_base_secs {
            return Err((
                RETRY_CAP_ENV,
                format!(
                    "{} is below {RETRY_BASE_ENV} ({}). Set it to at least that, or unset it \
                     for the default, {}",
                    s.retry_cap_secs, s.retry_base_secs, d.retry_cap_secs
                ),
            ));
        }
        Ok(s)
    }

    /// The wait after try number `failed` (from 1) failed: the base, doubled
    /// for each try before it, never more than the cap.
    pub fn backoff(&self, failed: u64) -> u64 {
        let doublings = failed.saturating_sub(1).min(32) as u32;
        self.retry_base_secs
            .saturating_mul(1u64 << doublings)
            .min(self.retry_cap_secs)
    }
}

/// **The own tenants being set up, as this server holds them**, so the door
/// can answer `tenant_not_ready` with a map read.
///
/// Keyed by tenant id, holding the person it is for. Filled when a sign-up
/// reserves a tenant here, and replaced from the registry by every sweep, so
/// a server that did not take the sign-up learns of it within one sweep.
#[derive(Debug)]
pub struct NotReady {
    held: RwLock<HashMap<String, String>>,
    retry_secs: u64,
}

impl NotReady {
    pub fn new(retry_secs: u64) -> NotReady {
        NotReady {
            held: RwLock::new(HashMap::new()),
            retry_secs: retry_secs.max(1),
        }
    }

    pub fn hold(&self, space: &str, principal: &str) {
        if let Ok(mut held) = self.held.write() {
            held.insert(space.to_string(), principal.to_string());
        }
    }

    pub fn lift(&self, space: &str) {
        if let Ok(mut held) = self.held.write() {
            held.remove(space);
        }
    }

    pub fn replace(&self, now: HashMap<String, String>) {
        if let Ok(mut held) = self.held.write() {
            *held = now;
        }
    }

    /// The person `space` is being set up for, when it is.
    pub fn principal_of(&self, space: &str) -> Option<String> {
        self.held.read().ok()?.get(space).cloned()
    }

    /// The `Retry-After` wait, in seconds.
    pub fn retry_secs(&self) -> u64 {
        self.retry_secs
    }
}

/// What one sweep found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Swept {
    /// Own tenants still being set up.
    pub in_progress: u64,
    /// Of those, how many are past `ORDO_PROVISION_STUCK_SECS`.
    pub stuck: u64,
    /// Setups this server took and started.
    pub started: u64,
}

/// **The sweep, on every running server** (owner, 2026-10-21).
///
/// Not a leader's job: any server that is running keeps setups going, and
/// the registry's lease decides which one runs each. Each server also
/// refreshes its own [`NotReady`] map and the two gauges.
pub struct TenantSetupSweep {
    pub ops: Arc<TenantOps>,
}

impl Worker for TenantSetupSweep {
    fn name(&self) -> &'static str {
        "tenants.setup.sweep"
    }

    fn cadence(&self) -> Cadence {
        Cadence::Tick(Duration::from_secs(self.ops.setup().sweep_secs))
    }

    fn requires_leadership(&self) -> bool {
        false
    }

    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let swept = self
                .ops
                .sweep_setups(ctx.now())
                .await
                .map_err(|e| WorkerError(e.to_string()))?;
            ctx.metrics
                .gauge(IN_PROGRESS_METRIC, &[], swept.in_progress);
            ctx.metrics.gauge(STUCK_METRIC, &[], swept.stuck);
            Ok(if swept.started == 0 {
                Progress::idle()
            } else {
                Progress::did(swept.started)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_hold_when_nothing_is_set() {
        let s = SetupSettings::from_vars(&|_| None).expect("defaults");
        assert_eq!(s, SetupSettings::default());
        assert_eq!(s.stuck_secs, 300);
    }

    #[test]
    fn a_value_out_of_range_names_the_setting_and_the_default() {
        let (var, why) = SetupSettings::from_vars(&|k| (k == STUCK_ENV).then(|| "0".to_string()))
            .expect_err("refused");
        assert_eq!(var, STUCK_ENV);
        assert!(why.contains("300"), "{why}");
        let (var, _) = SetupSettings::from_vars(&|k| match k {
            RETRY_BASE_ENV => Some("60".into()),
            RETRY_CAP_ENV => Some("30".into()),
            _ => None,
        })
        .expect_err("cap below base");
        assert_eq!(var, RETRY_CAP_ENV);
    }

    #[test]
    fn the_wait_doubles_and_stops_at_the_cap() {
        let s = SetupSettings::default();
        assert_eq!(s.backoff(1), 15);
        assert_eq!(s.backoff(2), 30);
        assert_eq!(s.backoff(3), 60);
        assert_eq!(s.backoff(7), 900);
        assert_eq!(s.backoff(1_000), 900);
    }
}
