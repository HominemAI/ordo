//! Settings an upgrade tightened: refused for a new deployment, warned for
//! a running one (owner ruling 2026-11-02, board task
//! `the-live-deployment-stopped-starting-after`).
//!
//! The standing rule is "upgrade should affect nothing". A deployment that
//! was serving under a setting this release now refuses must keep serving
//! after the upgrade, with a loud warning. A deployment that has never run
//! is refused as before, because nothing is lost by fixing it first.
//!
//! [`crate::runtime::RuntimeConfig::from_env`] records each tightened
//! setting it met as a [`Tightened`] instead of refusing. The boot settles
//! them with [`settle`] once the control database is reachable and before
//! custody mints anything. What survives is printed in the startup log and
//! reported under `warnings` on `/health/deep`, which the console's Health
//! screen shows.
//!
//! A deployment counts as running when `$keycustody` holds a wrapped space
//! key ([`ordo_exec::keystore::holds_custody`]). Every boot mints the
//! system space's key and every `ordo keys init` mints a tenant's, so a
//! deployment that ever served has one. The blind-index probe in
//! `build_custody` still checks that this master opens them.

use serde_json::{json, Value};

use crate::runtime::BootRefusal;

/// One setting an upgrade tightened, as this deployment has it set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tightened {
    /// The variable, named in the refusal and the warning.
    pub var: &'static str,
    /// The value this deployment set.
    pub value: String,
    /// When the release started refusing it, as a date.
    pub since: &'static str,
    /// Why it is refused and how to fix it: the refusal's own text.
    pub detail: String,
}

impl Tightened {
    pub fn new(
        var: &'static str,
        value: impl Into<String>,
        since: &'static str,
        detail: impl Into<String>,
    ) -> Tightened {
        Tightened {
            var,
            value: value.into(),
            since,
            detail: detail.into(),
        }
    }

    /// What a new deployment is told. The same text the boot gave before
    /// the ruling, so the setting and the fix are named the same way.
    pub fn refusal(&self) -> BootRefusal {
        BootRefusal::Invalid {
            var: self.var,
            detail: format!("`{}`: {}", self.value, self.detail),
        }
    }

    /// What a running deployment is told, in the log and on `/health/deep`.
    pub fn warning(&self) -> String {
        format!(
            "{}=`{}` is refused for a new deployment since {}. This deployment already \
             holds data, so it keeps serving. {}",
            self.var, self.value, self.since, self.detail
        )
    }

    /// The `/health/deep` entry.
    pub fn json(&self) -> Value {
        json!({
            "setting": self.var,
            "value": self.value,
            "since": self.since,
            "message": self.warning(),
        })
    }
}

/// Settle every tightened setting once whether this deployment is running
/// is known: a new one is refused by the first, a running one gets them
/// all back as warnings, each printed loudly on stderr.
pub fn settle(pending: &[Tightened], running: bool) -> Result<Vec<Tightened>, BootRefusal> {
    if let Some(first) = pending.first().filter(|_| !running) {
        return Err(first.refusal());
    }
    for t in pending {
        eprintln!("ordo-host: WARNING: {}", t.warning());
    }
    Ok(pending.to_vec())
}

/// [`settle`], reading whether the deployment is running from the control
/// database. No query when nothing is pending.
pub async fn settle_on(
    pending: &[Tightened],
    pool: &ordo_exec::Pool,
) -> Result<Vec<Tightened>, BootRefusal> {
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let running = ordo_exec::keystore::holds_custody(pool)
        .await
        .map_err(BootRefusal::Custody)?;
    settle(pending, running)
}

/// What `ordo-host check-config` says about a tightened setting. It never
/// dials, so it cannot tell a running deployment from a new one, and it
/// passes: refusing would block every apply to a running deployment.
pub fn check_config_note(t: &Tightened) -> String {
    format!(
        "WARNING: {}=`{}` boots only a deployment that already holds data; a new one is \
         refused. {}",
        t.var, t.value, t.detail
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Tightened {
        Tightened::new(
            "ORDO_KEY_PROVIDER",
            "local",
            "2026-09-29",
            "use sealed-master",
        )
    }

    #[test]
    fn a_new_deployment_is_refused_by_name() {
        let e = settle(&[key()], false).expect_err("a new deployment was admitted");
        let text = e.to_string();
        assert!(text.starts_with("ORDO_KEY_PROVIDER: `local`"), "{text}");
        assert!(text.contains("use sealed-master"), "{text}");
    }

    #[test]
    fn a_running_deployment_keeps_every_warning() {
        let kept = settle(&[key()], true).expect("a running deployment was refused");
        assert_eq!(kept, vec![key()]);
        let j = kept[0].json();
        assert_eq!(j["setting"], "ORDO_KEY_PROVIDER");
        assert!(j["message"].as_str().unwrap().contains("use sealed-master"));
    }

    #[test]
    fn nothing_pending_settles_to_nothing() {
        assert_eq!(settle(&[], false).expect("nothing to refuse"), vec![]);
    }
}
