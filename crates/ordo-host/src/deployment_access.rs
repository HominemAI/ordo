//! The realm boundary at the door (`plan-identity-service.md` 4.3): what a
//! verified deployment token may reach.
//!
//! | Caller | Platform op, home space | [`TENANT_OPS_OPEN`] in a tenant | App op, any space | Other op in a tenant |
//! |---|---|---|---|---|
//! | Person, touch fresh | allowed | allowed | denied | denied |
//! | Person, touch stale | reads allowed, a write the touch gate names: `presence_required` | `presence_required` | denied | denied |
//! | Machine key | allowed, writes included | denied | denied | denied |
//!
//! A break-glass token is a person here: it carries a fresh touch.
//!
//! "A write the touch gate names" is the deployment's own presence policy
//! for platform ops (`presence.rs`, `ORDO_STEP_UP_EXEMPT_OPS`,
//! `ORDO_PRESENCE_ON_READS`), as the access engine holds it: the same gate
//! an app session meets, with `touched_at` as its presence. A machine key
//! needs no touch in the home space (owner, 2026-10-27, Q2).
//!
//! A tenant op asks for a touch in the last [`TENANT_REACH_SECS`], read or
//! write, as a console key session's tenant reach does today.
//!
//! [`decide`] is the one decision. `IdentityAccess::check` runs it, and
//! `IdentityAccess::presence_required` asks it again after a denial.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use ordo_identity::deployment_token::{DeploymentClaims, DeploymentTokenError, Kind};
use ordo_secevent::{RecordedDecision, Subject, TokenFacts};

pub use crate::console_keys::{TENANT_OPS_OPEN, TENANT_REACH_SECS};

/// How far a token's `touched_at` may sit ahead of the door's clock.
const TOUCH_LEEWAY_SECS: u64 = ordo_identity::deployment_token::DEFAULT_LEEWAY_SECS;

/// The platform surfaces outside the op catalog that the door also checks
/// by name (`serve::authorize_in`): health, metrics, alerts, the security
/// record, usage.
const PLATFORM_SURFACES: [&str; 5] = [
    crate::serve::HEALTH_DEEP_OP,
    crate::serve::METRICS_OP,
    crate::serve::ALERTS_OP,
    crate::serve::SECEVENTS_OP,
    crate::serve::USAGE_OP,
];

static PLATFORM_OPS: LazyLock<BTreeSet<String>> = LazyLock::new(|| {
    crate::platform_ops::specs()
        .into_iter()
        .map(|s| s.name)
        .chain(PLATFORM_SURFACES.iter().map(|s| s.to_string()))
        .collect()
});

/// Is `op` one of this build's platform ops? Built once.
pub fn is_platform_op(op: &str) -> bool {
    PLATFORM_OPS.contains(op)
}

/// What [`decide`] answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Realm {
    Allow,
    /// Allowed once the person touches again. Recorded as `StepUpStale`;
    /// the wire answers `presence_required`.
    PresenceRequired,
    /// Outside the realm. Recorded as [`RecordedDecision::OutsideDeploymentRealm`];
    /// the wire answers `access_denied`.
    Deny,
}

/// Was the touch at `touched_at` within `window` seconds of `now`?
fn fresh(touched_at: Option<u64>, window: u64, now: u64) -> bool {
    touched_at.is_some_and(|t| {
        t <= now.saturating_add(TOUCH_LEEWAY_SECS) && now.saturating_sub(t) <= window
    })
}

/// The realm table. `home` is the deployment's own space (`None` when the
/// door was not told it: then nothing is reached). `touch_window` is the
/// touch the op asks of a person in the home space, `None` for none.
pub fn decide(
    claims: &DeploymentClaims,
    home: Option<&str>,
    space: &str,
    op: &str,
    touch_window: Option<u64>,
    now: u64,
) -> Realm {
    let Some(home) = home else {
        return Realm::Deny;
    };
    if !is_platform_op(op) {
        return Realm::Deny;
    }
    let person = matches!(claims.kind, Kind::Person | Kind::Breakglass);
    if space == home {
        return match (person, touch_window) {
            (false, _) | (true, None) => Realm::Allow,
            (true, Some(window)) if fresh(claims.touched_at, window, now) => Realm::Allow,
            (true, Some(_)) => Realm::PresenceRequired,
        };
    }
    if !person || !TENANT_OPS_OPEN.contains(&op) {
        return Realm::Deny;
    }
    if fresh(claims.touched_at, TENANT_REACH_SECS, now) {
        Realm::Allow
    } else {
        Realm::PresenceRequired
    }
}

/// The security record's word for a deployment token that did not verify.
pub fn refused_as(e: &DeploymentTokenError) -> RecordedDecision {
    match e {
        DeploymentTokenError::WrongIssuer => RecordedDecision::DeploymentTokenWrongIssuer,
        DeploymentTokenError::WrongAudience => RecordedDecision::DeploymentTokenWrongAudience,
        DeploymentTokenError::Expired => RecordedDecision::DeploymentTokenExpired,
        DeploymentTokenError::Unverifiable(_)
        | DeploymentTokenError::NotYetValid
        | DeploymentTokenError::Malformed(_) => RecordedDecision::DeploymentTokenRefused,
    }
}

/// The security record's subject for a verified deployment token: `sub`,
/// with the token's `kind`, `client` and `sid` beside it (plan section 7).
pub fn subject(claims: &DeploymentClaims) -> Subject {
    Subject::deployment(
        &claims.sub,
        TokenFacts {
            kind: claims.kind.as_str().to_string(),
            client: claims.client.clone(),
            sid: claims.sid.clone(),
        },
    )
}

/// The metric's principal kind for a deployment token's holder.
pub fn metric_kind(claims: &DeploymentClaims) -> &'static str {
    match claims.kind {
        Kind::Person => "deployment-person",
        Kind::Machine => "deployment-machine",
        Kind::Breakglass => "deployment-breakglass",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_identity::deployment_token::{audience, ROLE_ADMIN};

    const NOW: u64 = 10_000;
    const HOME: &str = "system";
    const APPLY: &str = "std.deployment.apply";

    fn person(touched_at: Option<u64>) -> DeploymentClaims {
        DeploymentClaims {
            iss: "https://id.example.com".into(),
            aud: audience("main"),
            sub: "p_1".into(),
            kind: Kind::Person,
            role: ROLE_ADMIN.into(),
            client: "console".into(),
            sid: "s_1".into(),
            touched_at,
            iat: NOW,
            exp: NOW + 600,
            jti: "j".into(),
        }
    }

    #[test]
    fn every_tenant_open_op_is_a_platform_op() {
        for op in TENANT_OPS_OPEN {
            assert!(is_platform_op(op), "{op}");
        }
        assert!(is_platform_op(APPLY));
        assert!(!is_platform_op("notes.create"));
    }

    #[test]
    fn the_table_cell_by_cell() {
        let fresh = person(Some(NOW - 10));
        let stale = person(Some(NOW - 1_000));
        let mut machine = person(None);
        machine.kind = Kind::Machine;
        machine.sub = "m_ci".into();
        let tenant_op = TENANT_OPS_OPEN[0];
        let d = |c: &DeploymentClaims, space: &str, op: &str, w: Option<u64>| {
            decide(c, Some(HOME), space, op, w, NOW)
        };
        assert_eq!(d(&fresh, HOME, APPLY, Some(300)), Realm::Allow);
        assert_eq!(d(&stale, HOME, APPLY, Some(300)), Realm::PresenceRequired);
        assert_eq!(d(&stale, HOME, APPLY, None), Realm::Allow);
        assert_eq!(d(&machine, HOME, APPLY, Some(300)), Realm::Allow);
        assert_eq!(d(&fresh, "t1", tenant_op, None), Realm::Allow);
        assert_eq!(d(&stale, "t1", tenant_op, None), Realm::PresenceRequired);
        assert_eq!(d(&machine, "t1", tenant_op, None), Realm::Deny);
        assert_eq!(d(&fresh, "t1", APPLY, None), Realm::Deny);
        for c in [&fresh, &stale, &machine] {
            assert_eq!(d(c, HOME, "notes.create", None), Realm::Deny);
            assert_eq!(d(c, "t1", "notes.create", None), Realm::Deny);
        }
        assert_eq!(decide(&fresh, None, HOME, APPLY, None, NOW), Realm::Deny);
        // A touch from the future past the leeway is not a touch.
        let ahead = person(Some(NOW + 600));
        assert_eq!(d(&ahead, HOME, APPLY, Some(300)), Realm::PresenceRequired);
    }
}
