//! **Moving tenants off a revoked version** (`plan-versions.md` §1.11,
//! increment 3 session 2; owner rulings of 2026-11-05).
//!
//! # What the owner ruled
//!
//! - No deadline. A tenant on a revoked version moves in its maintenance
//!   window, or on demand. With no window and no demand it stays.
//! - On demand is the deployment owner's act, or the tenant's own when the
//!   app developer grants it. Off by default.
//! - The target is the app developer's choice: the tenant's own, the
//!   revoke's `--move-to`, else the active version. Never a revoked one.
//! - The developer's own system may drive moves through the door, with no
//!   passkey touch. Nor is a person asked for one.
//!
//! # The three paths, one move
//!
//! Each calls [`Versions::move_off_revoked`], which reads the target and
//! moves the pin in one transaction under the pin lock.
//!
//! | Path | Who | Cause on `$pins` |
//! |---|---|---|
//! | [`sweep`], on each node's pin reconcile tick | Ordo, in a window | `window` |
//! | `std.deployment.moveOffRevoked($tenant)` | the deployment owner, or a system's machine key | `demand` |
//! | `std.space.moveOffRevoked`, bound to the tenant | the tenant roles in [`ROLES_ENV`], none by default | `tenant` |
//!
//! Every node sweeps. Two nodes that find the same tenant in its window
//! move it once: the second is told it is no longer on a revoked version,
//! which the sweep counts as nothing to do.

use std::sync::OnceLock;

use ordo_api::{
    BoundParams, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec, RefineTier,
    Syncability,
};
use ordo_exec::Pool;
use ordo_identity::principal::Role;
use serde_json::json;

use crate::apply_op::ApplyOps;
use crate::versions::{MovedOff, PinCause, VersionRefusal, Versions};
use crate::windows::Windows;

/// The tenant moves itself off a revoked version. Granted to the roles
/// [`ROLES_ENV`] names, none by default.
pub const MOVE_SELF: &str = "std.space.moveOffRevoked";

pub const OPS: [&str; 1] = [MOVE_SELF];

/// The setting naming the tenant roles that may move their own tenant off
/// a revoked version.
pub const ROLES_SETTING: &str = "versions.tenant_move_roles";
/// Its environment variable. Unset or empty: no tenant role may.
pub const ROLES_ENV: &str = "ORDO_VERSIONS_TENANT_MOVE_ROLES";

/// How many tenants one sweep moves at most. The rest move on the next
/// tick while their window holds.
pub const SWEEP_BATCH: usize = 1_000;

/// The actor a window move is recorded under, before the window's id.
pub const WINDOW_ACTOR: &str = "ordo:window";

// ── who may, inside a tenant ─────────────────────────────────────────────

/// **Which tenant roles may move their own tenant off a revoked version**
/// (owner, 2026-11-05: *"optionally on demand which may be triggered by
/// the tenant if the owner provides the means to the tenant"*). The app
/// developer's setting. Nobody by default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MovePolicy {
    roles: Vec<Role>,
}

impl MovePolicy {
    /// From a comma list of roles, as [`ROLES_ENV`] holds it.
    pub fn parse(list: Option<&str>) -> Result<MovePolicy, String> {
        Ok(MovePolicy {
            roles: crate::pause::roles(list.unwrap_or(""), ROLES_ENV)?,
        })
    }

    /// From [`ROLES_ENV`].
    pub fn from_env() -> Result<MovePolicy, String> {
        MovePolicy::parse(std::env::var(ROLES_ENV).ok().as_deref())
    }

    /// The roles granted `op`: those named for [`MOVE_SELF`], `None` for
    /// any other op.
    pub fn roles_for(&self, op: &str) -> Option<&[Role]> {
        (op == MOVE_SELF).then_some(self.roles.as_slice())
    }

    /// The census line.
    pub fn describe(&self) -> String {
        if self.roles.is_empty() {
            format!("nobody in a tenant ({ROLES_ENV} unset)")
        } else {
            self.roles
                .iter()
                .map(|r| format!("{r:?}").to_lowercase())
                .collect::<Vec<_>>()
                .join(",")
        }
    }
}

static INSTALLED: OnceLock<MovePolicy> = OnceLock::new();

/// Install the deployment's policy, once, before any engine is built.
pub fn install(policy: MovePolicy) {
    let _ = INSTALLED.set(policy);
}

/// The policy engines are built under: what was installed, or nobody.
pub fn current() -> MovePolicy {
    INSTALLED.get().cloned().unwrap_or_default()
}

// ── the window sweep ─────────────────────────────────────────────────────

/// What one [`sweep`] did.
#[derive(Debug, Default)]
pub struct Swept {
    /// Tenants moved, each with its window's id.
    pub moved: Vec<(MovedOff, String)>,
    /// Tenants in a window that did not move, with why. Each stays where
    /// it is and is tried again on the next tick while the window holds.
    pub refused: Vec<(String, String)>,
}

/// **Move every tenant on a revoked version whose maintenance window holds
/// at `now`.** A tenant no window holds stays. Run on each node's pin
/// reconcile tick (`pins.reconcile_interval`).
pub async fn sweep(pool: &Pool, now: u64) -> Result<Swept, VersionRefusal> {
    sweep_under(pool, crate::versions::policy(), now).await
}

/// [`sweep`] under a named `versions.served` and `versions.minimum`: a
/// tenant below the minimum moves in its window too (§1.6).
pub async fn sweep_under(
    pool: &Pool,
    policy: &crate::versions::ServedPolicy,
    now: u64,
) -> Result<Swept, VersionRefusal> {
    let mut out = Swept::default();
    let v = Versions::on(pool).with_policy(policy);
    let on = v.on_revoked(SWEEP_BATCH).await?;
    if on.is_empty() {
        return Ok(out);
    }
    let open = match Windows::on(pool).open_at(now as i64).await {
        Ok(o) => o,
        Err(e) => {
            out.refused
                .push(("*".into(), format!("the windows were not read: {e}")));
            return Ok(out);
        }
    };
    if open.is_empty() {
        return Ok(out);
    }
    for (space, _) in on {
        let Some((w, _, _)) = open.iter().find(|(w, _, _)| w.target.covers(&space)) else {
            continue;
        };
        let actor = format!("{WINDOW_ACTOR}:{}", w.id);
        match v
            .move_off_revoked(&space, &actor, PinCause::Window, now)
            .await
        {
            Ok(m) => out.moved.push((m, w.id.clone())),
            // Another node moved it first.
            Err(VersionRefusal::NotOnRevoked { .. }) | Err(VersionRefusal::NoTenant { .. }) => {}
            Err(e) => out.refused.push((space, e.to_string())),
        }
    }
    Ok(out)
}

/// Fill each listed tenant's next window start (`RevokedPin::next_window`),
/// so the report says when it will move. Best effort: windows that cannot
/// be read leave every tenant at "no window" and the report still answers.
pub async fn note_next_windows(
    pool: &Pool,
    list: &mut [crate::versions::RevokedVersion],
    now: u64,
) {
    let spaces: Vec<String> = list
        .iter()
        .flat_map(|r| r.tenants.iter().map(|t| t.space.clone()))
        .collect();
    let refs: Vec<&str> = spaces.iter().map(String::as_str).collect();
    let Ok(next) = Windows::on(pool).next_for(&refs, now as i64).await else {
        return;
    };
    for t in list.iter_mut().flat_map(|r| r.tenants.iter_mut()) {
        t.next_window = next.get(&t.space).copied();
    }
}

/// One line per thing a sweep did, for the node's log.
pub fn log_lines(s: &Swept) -> Vec<String> {
    let mut out = Vec::new();
    for (m, window) in &s.moved {
        out.push(format!(
            "versions: tenant {} moved off revoked {} to {} in window {window} (target: {})",
            m.change.space,
            m.change.from.as_deref().unwrap_or("none"),
            m.change.to,
            m.source.as_str()
        ));
    }
    for (space, why) in &s.refused {
        out.push(format!(
            "versions: tenant {space} is in its window and stays on a revoked version: {why}"
        ));
    }
    out
}

// ── the tenant's own op ──────────────────────────────────────────────────

/// The spec of [`MOVE_SELF`].
pub fn specs() -> Vec<OpSpec> {
    vec![OpSpec {
        name: MOVE_SELF.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind: OpKind::Write,
        params: vec![],
        output_fields: ["tenant", "from", "to", "epoch", "ordinal", "chosenBy"]
            .iter()
            .map(|f| FieldSpec::new(*f))
            .collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }]
}

/// Run [`MOVE_SELF`] for the tenant `space` the call is bound to. The door
/// has granted it only to the roles [`ROLES_ENV`] names.
pub async fn execute(
    ops: &ApplyOps,
    space: &str,
    _bound: &BoundParams,
    caller: &str,
) -> Result<ExecResult, ExecError> {
    let moved = Versions::on(&ops.pool)
        .move_off_revoked(space, caller, PinCause::Tenant, crate::runtime::now_secs())
        .await
        .map_err(|e| e.to_exec_error("the tenant was not moved"))?;
    ops.record(
        caller,
        MOVE_SELF,
        "moved-off-revoked",
        format!(
            "tenant={space} version={} was={} by={caller}",
            moved.change.to,
            moved.change.from.as_deref().unwrap_or("none")
        ),
    );
    Ok(ExecResult {
        position: None,
        rows: vec![moved_json(&ops.pool, &moved).await?],
        next_cursor: None,
    })
}

/// What a move off a revoked version answers at the door.
pub async fn moved_json(pool: &Pool, m: &MovedOff) -> Result<serde_json::Value, ExecError> {
    let ordinal = Versions::on(pool)
        .ordinal_of(&m.change.to)
        .await
        .map_err(|e| e.to_exec_error("the version's order number could not be read"))?;
    Ok(json!({
        "tenant": m.change.space,
        "from": m.change.from,
        "to": m.change.to,
        "epoch": m.change.epoch,
        "ordinal": ordinal,
        "chosenBy": m.source.as_str(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nobody_in_a_tenant_may_unless_the_developer_names_roles() {
        let p = MovePolicy::default();
        assert_eq!(p.roles_for(MOVE_SELF), Some(&[][..]));
        assert_eq!(p.roles_for("std.space.pause"), None);
        let p = MovePolicy::parse(Some("owner, manager")).expect("parses");
        assert_eq!(
            p.roles_for(MOVE_SELF),
            Some(&[Role::Owner, Role::Manager][..])
        );
        let e = MovePolicy::parse(Some("admin")).unwrap_err();
        assert!(e.contains(ROLES_ENV) && e.contains("admin"), "{e}");
    }
}
