//! **Removing a stored version** (`plan-versions.md` §1.6, §2.7; ruled
//! 2026-10-01 and 2026-11-02, D15): `ordo versions remove <version>
//! [--move-to <version>] --reason "..."`, door op
//! `std.deployment.removeVersion`.
//!
//! The owner, 2026-10-01: *"No need to get rid of artifacts at all unless
//! app dev wants to then."* So nothing is removed on its own, and a removal
//! moves only the tenants the deployment owner named a version for.
//!
//! # What a removal does
//!
//! 1. Every check below runs first. A refusal moves no tenant.
//! 2. Each tenant pinned to the version is brought to `--move-to`: a
//!    database of its own is brought forward and gated, the deployment's
//!    own must hold it already (the same step a tenant build takes,
//!    `crate::tenant_builds::ready_for`). Every tenant is ready before any
//!    pin moves.
//! 3. Each is pinned there, cause `remove`.
//! 4. Under the pin lock the version's row becomes a tombstone
//!    ([`ordo_exec::registry::mark_removed`]): name, order number and
//!    fingerprint stay, the program is cleared. Storing the same program
//!    again brings it back.
//!
//! # What it refuses, by name
//!
//! - the active version (`remove_active`);
//! - a version a tenant build serves (`remove_in_build`);
//! - a version a setting or a tenant's move target names
//!   (`remove_named_by_setting`);
//! - a version a rollout, a move or the deployment's own space is on its
//!   way to or from (`remove_in_motion`);
//! - a version a restore may still need (`remove_restorable`, increment
//!   17): it served within `versions.restore_window`, or ever, when that is
//!   unset;
//! - tenants on it and no `--move-to` (`remove_has_tenants`);
//! - a tenant that cannot move to `--move-to` (`remove_cannot_move`).

use ordo_api::{ErrorCode, ExecError};
use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_fault::FaultClass;

use crate::apply_op::ApplyOps;
use crate::tenant_builds::{BuildRefusal, TenantBuilds};
use crate::versions::{PinCause, RestoreHold, VersionRefusal, VersionSetting, Versions};

/// The door op.
pub const REMOVE_VERSION: &str = "std.deployment.removeVersion";

/// What a bring forward on a tenant's own database records as its op.
const OP: &str = "version_removal";

/// Why a version was not removed. The one typed error of a removal (rule
/// 9); [`RemoveRefusal::to_exec_error`] is where it becomes a door answer.
#[derive(Debug)]
pub enum RemoveRefusal {
    /// The active version is never removed.
    Active { named: String },
    /// A tenant build serves it.
    InBuild { named: String, build: String },
    /// A setting, or a tenant's move target, names it.
    NamedBySetting {
        named: String,
        what: String,
        fix: String,
    },
    /// A rollout, a move or the deployment's own space is on its way to
    /// or from it.
    InMotion {
        named: String,
        what: String,
        fix: String,
    },
    /// A restore may still need it (increment 17).
    Restorable(RestoreHold),
    /// Tenants are pinned to it, and no `--move-to` named where they go.
    HasTenants { named: String, tenants: Vec<String> },
    /// A tenant could not be moved to `--move-to`. No tenant moved, unless
    /// `moved` says so (a pin refused after the others moved).
    CannotMove {
        named: String,
        space: String,
        to: String,
        why: String,
        transient: bool,
        moved: Vec<String>,
    },
    /// The version or `--move-to` refused as a pin would.
    Version(VersionRefusal),
    /// Another apply holds the apply lock.
    Busy,
    /// The database.
    Store(PoolError),
}

impl From<PoolError> for RemoveRefusal {
    fn from(e: PoolError) -> Self {
        RemoveRefusal::Store(e)
    }
}

impl From<VersionRefusal> for RemoveRefusal {
    fn from(e: VersionRefusal) -> Self {
        match e {
            VersionRefusal::Store(e) => RemoveRefusal::Store(e),
            other => RemoveRefusal::Version(other),
        }
    }
}

impl RemoveRefusal {
    /// The greppable class.
    pub fn class(&self) -> &'static str {
        match self {
            RemoveRefusal::Active { .. } => "remove_active",
            RemoveRefusal::InBuild { .. } => "remove_in_build",
            RemoveRefusal::NamedBySetting { .. } => "remove_named_by_setting",
            RemoveRefusal::InMotion { .. } => "remove_in_motion",
            RemoveRefusal::Restorable(_) => "remove_restorable",
            RemoveRefusal::HasTenants { .. } => "remove_has_tenants",
            RemoveRefusal::CannotMove { .. } => "remove_cannot_move",
            RemoveRefusal::Version(_) => "version_refused",
            RemoveRefusal::Busy => "apply_in_progress",
            RemoveRefusal::Store(_) => "store_failed",
        }
    }

    /// **The one place a `RemoveRefusal` becomes a door answer.**
    pub fn to_exec_error(&self) -> ExecError {
        let what = "the version was not removed";
        match self {
            RemoveRefusal::Version(e) => e.to_exec_error(what),
            RemoveRefusal::Store(e) => ExecError::of_fault(
                e.class(),
                ErrorCode::SpaceUnavailable,
                format!("{what}: {self}"),
            ),
            RemoveRefusal::Busy
            | RemoveRefusal::CannotMove {
                transient: true, ..
            } => ExecError::of_fault(
                FaultClass::Busy,
                ErrorCode::SpaceUnavailable,
                format!("{what}: {self}"),
            ),
            _ => ExecError::new(ErrorCode::ApplyRefused, format!("{what}: {self}")),
        }
    }
}

impl std::fmt::Display for RemoveRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let class = self.class();
        match self {
            RemoveRefusal::Active { named } => write!(
                f,
                "{class}: {named} is the active version, and the active version is not removed. \
                 Activate another first (`ordo apply <dir>` or `ordo rollback <version>`), then \
                 remove it"
            ),
            RemoveRefusal::InBuild { named, build } => write!(
                f,
                "{class}: tenant build `{build}` serves {named}. Set the build to another \
                 version (`ordo tenant-builds set {build} <version> --reason \"...\"`) or remove \
                 it, then remove the version"
            ),
            RemoveRefusal::NamedBySetting { named, what, fix } => {
                write!(f, "{class}: {what} names {named}. {fix}, then remove it")
            }
            RemoveRefusal::InMotion { named, what, fix } => {
                write!(f, "{class}: {what} {named}. {fix}, then remove it")
            }
            RemoveRefusal::Restorable(hold) => write!(f, "{class}: {}", hold.line()),
            RemoveRefusal::HasTenants { named, tenants } => write!(
                f,
                "{class}: {} tenant{} pinned to {named}: {}. Name the version they move to: \
                 `ordo versions remove {named_short} --move-to <version> --reason \"...\"`",
                tenants.len(),
                if tenants.len() == 1 { " is" } else { "s are" },
                shortlist(tenants),
                named_short = short(named),
            ),
            RemoveRefusal::CannotMove {
                named,
                space,
                to,
                why,
                transient,
                moved,
            } => {
                write!(
                    f,
                    "{class}: tenant {space} cannot move to {to}: {why}. {}",
                    if *transient {
                        "It could not be reached; try again".to_string()
                    } else {
                        format!(
                            "Move it yourself (`ordo evolve --tenant {space} --to <version>` or \
                             `ordo versions pin {space} <version>`), or name another \
                             `--move-to`"
                        )
                    }
                )?;
                if moved.is_empty() {
                    write!(f, ". No tenant moved, and {named} is still stored")
                } else {
                    write!(
                        f,
                        ". {} moved to {to} before it ({}), and {named} is still stored",
                        moved.len(),
                        shortlist(moved)
                    )
                }
            }
            RemoveRefusal::Version(e) => write!(f, "{e}"),
            RemoveRefusal::Busy => write!(
                f,
                "{class}: an apply holds the apply lock, and a tenant's database is brought \
                 forward only under it. Try again when it is done"
            ),
            RemoveRefusal::Store(e) => write!(f, "the control database: {e}"),
        }
    }
}

/// `#4 · 0.2.0` read back as `#4`, the shortest name a person types.
fn short(named: &str) -> &str {
    named.split(" · ").next().unwrap_or(named)
}

/// Up to 5 names, then how many more.
fn shortlist(items: &[String]) -> String {
    let mut out = items.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > 5 {
        out.push_str(&format!(" and {} more", items.len() - 5));
    }
    out
}

/// What a removal did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    pub version: String,
    pub ordinal: Option<u64>,
    /// Where its tenants went, when there were any.
    pub moved_to: Option<String>,
    /// The tenants moved, by id.
    pub moved: Vec<String>,
}

/// **Remove `name`**, moving its tenants to `move_to` first. See the
/// module doc for the order and the refusals.
pub async fn remove(
    ops: &ApplyOps,
    name: &str,
    move_to: Option<&str>,
    reason: &str,
    actor: &str,
    at: u64,
) -> Result<Removed, RemoveRefusal> {
    let reason = crate::versions::check_reason(reason)?;
    let pool = &ops.pool;
    let v = Versions::on(pool);
    v.ensure().await?;
    let version = v.resolve(name).await?;
    let named = v.named(&version).await?;
    let ordinal = v.ordinal_of(&version).await?;
    if v.active_version().await?.as_deref() == Some(version.as_str()) {
        return Err(RemoveRefusal::Active { named });
    }
    let builds = TenantBuilds::on(ops)
        .list()
        .await
        .map_err(|e| RemoveRefusal::Store(PoolError::db(e.to_string())))?;
    if let Some(b) = builds.iter().find(|b| b.version == version) {
        return Err(RemoveRefusal::InBuild {
            named,
            build: b.name.clone(),
        });
    }
    named_by_settings(pool, &version, &named).await?;
    in_motion(ops, &version, &named).await?;
    if let Some(hold) = v.restore_hold(&version, at).await? {
        return Err(RemoveRefusal::Restorable(hold));
    }
    let tenants = pinned_to(ops, &version).await?;
    let mut moved = Vec::new();
    let mut to = None;
    if !tenants.is_empty() {
        let Some(m) = move_to.map(str::trim).filter(|m| !m.is_empty()) else {
            return Err(RemoveRefusal::HasTenants { named, tenants });
        };
        let target = v.resolve(m).await?;
        if target == version {
            return Err(VersionRefusal::MoveTo(format!(
                "it names {named}, the version being removed; name another"
            ))
            .into());
        }
        if let Some(mark) = v.revocation_of(&target).await? {
            return Err(VersionRefusal::Revoked {
                named: v.named(&target).await?,
                mark: Box::new(mark),
                setting: None,
            }
            .into());
        }
        v.check_served(&target).await?;
        v.require_arrival(&target).await?;
        let to_named = v.named(&target).await?;
        // Every tenant ready before any pin moves.
        crate::tenant_builds::ready_for(ops, &tenants, &target, actor, OP)
            .await
            .map_err(|e| cannot_move(&named, &to_named, &tenants, e))?;
        for space in &tenants {
            match v.pin_as(space, &target, actor, at, PinCause::Remove).await {
                Ok(_) => moved.push(space.clone()),
                // Reaped meanwhile: nothing to move.
                Err(VersionRefusal::NoTenant { .. }) => {}
                Err(e) => {
                    return Err(RemoveRefusal::CannotMove {
                        named,
                        space: space.clone(),
                        to: to_named,
                        why: e.to_string(),
                        transient: false,
                        moved,
                    })
                }
            }
        }
        to = Some(target);
    } else if let Some(m) = move_to.map(str::trim).filter(|m| !m.is_empty()) {
        // Nothing to move, but a name that is not a version is still a
        // mistake worth saying.
        v.resolve(m).await?;
    }
    let left = v
        .remove_stored(&version, &reason, to.as_deref(), moved.len(), actor, at)
        .await?;
    if !left.is_empty() {
        // A tenant pinned to it while the others moved.
        return Err(RemoveRefusal::HasTenants {
            named,
            tenants: left,
        });
    }
    Ok(Removed {
        version,
        ordinal,
        moved_to: to,
        moved,
    })
}

/// A refusal of the step that readies every tenant, named for the tenant.
fn cannot_move(named: &str, to: &str, tenants: &[String], e: BuildRefusal) -> RemoveRefusal {
    let (space, why, transient) = match e {
        BuildRefusal::TenantBehind {
            space,
            why,
            transient,
            ..
        } => (space, why, transient),
        BuildRefusal::DeploymentBehind { why, .. } => (
            tenants
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            format!(
                "the deployment's own database, where it lives, does not hold it ({why}). \
                 `ordo apply <dir> --no-activate` stores the version and brings the database \
                 to it"
            ),
            false,
        ),
        BuildRefusal::Busy => return RemoveRefusal::Busy,
        BuildRefusal::Store(e) => return RemoveRefusal::Store(e),
        BuildRefusal::Version(e) => return e.into(),
        other => (String::new(), other.to_string(), false),
    };
    RemoveRefusal::CannotMove {
        named: named.to_string(),
        space,
        to: to.to_string(),
        why,
        transient,
        moved: Vec::new(),
    }
}

/// Every tenant pinned to `version`, by id.
async fn pinned_to(ops: &ApplyOps, version: &str) -> Result<Vec<String>, RemoveRefusal> {
    Ok(ops
        .pool
        .rows(&format!(
            "SELECT \"$space\" FROM \"$spaces\" WHERE \"version\" = {} \
             AND \"state\" IN ('provisioning', 'active', 'paused') ORDER BY \"$space\"",
            sql_lit(version)
        ))
        .await?
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect())
}

/// Whether the name a setting holds (a name or `#n`) is `version`.
async fn names(v: &Versions<'_>, name: &str, version: &str) -> bool {
    v.resolve(name).await.ok().as_deref() == Some(version)
}

/// Refused when a version setting, or a tenant's move target, names it.
async fn named_by_settings(pool: &Pool, version: &str, named: &str) -> Result<(), RemoveRefusal> {
    let v = Versions::on(pool);
    let v = &v;
    let settings = v.settings().await?;
    if let Some(n) = &settings.new_tenant {
        if names(v, n, version).await {
            return Err(RemoveRefusal::NamedBySetting {
                named: named.to_string(),
                what: format!("`{}`", VersionSetting::NewTenant.name()),
                fix: format!(
                    "Point it at another version (`{} <version> --reason \"...\"`) or clear it",
                    VersionSetting::NewTenant.command()
                ),
            });
        }
    }
    if let Some(m) = &settings.policy.minimum {
        if names(v, m, version).await {
            return Err(RemoveRefusal::NamedBySetting {
                named: named.to_string(),
                what: format!("`{}`", VersionSetting::Minimum.name()),
                fix: format!(
                    "Raise it to a later version (`{} <version> --reason \"...\"`) or clear it",
                    VersionSetting::Minimum.command()
                ),
            });
        }
    }
    if let Some(list) = &settings.policy.served {
        let mut rest = Vec::new();
        let mut found = false;
        for n in list {
            if names(v, n, version).await {
                found = true;
            } else {
                rest.push(n.clone());
            }
        }
        if found {
            return Err(RemoveRefusal::NamedBySetting {
                named: named.to_string(),
                what: format!("`{}`", VersionSetting::Served.name()),
                fix: format!(
                    "Leave it out (`{} {} --reason \"...\"`)",
                    VersionSetting::Served.command(),
                    if rest.is_empty() {
                        "in-use".to_string()
                    } else {
                        rest.join(",")
                    }
                ),
            });
        }
    }
    let targets = match pool
        .rows(&format!(
            "SELECT \"space\" FROM \"$move_targets\" WHERE \"version\" = {} ORDER BY \"space\"",
            sql_lit(version)
        ))
        .await
    {
        Ok(rows) => rows,
        Err(e) if e.is_missing_table() => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let targets: Vec<String> = targets
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();
    if let Some(first) = targets.first() {
        return Err(RemoveRefusal::NamedBySetting {
            named: named.to_string(),
            what: format!(
                "the move target of {} tenant{} ({})",
                targets.len(),
                if targets.len() == 1 { "" } else { "s" },
                shortlist(&targets)
            ),
            fix: format!("Clear or change each (`ordo versions target {first} --clear`)"),
        });
    }
    Ok(())
}

/// Refused while a rollout, a move or the deployment's own space is on
/// its way to or from it.
async fn in_motion(ops: &ApplyOps, version: &str, named: &str) -> Result<(), RemoveRefusal> {
    let refuse = |what: String, fix: &str| RemoveRefusal::InMotion {
        named: named.to_string(),
        what,
        fix: fix.to_string(),
    };
    if let Ok(Some(r)) = crate::rollouts::Rollouts::on(&ops.pool).open().await {
        if r.target == version {
            return Err(refuse(
                format!("rollout {} is moving tenants to", r.id),
                "Let it finish, or abort it (`ordo rollout status`, `ordo rollout abort`)",
            ));
        }
    }
    let moves = match ops
        .pool
        .rows(&format!(
            "SELECT \"space\" FROM \"$moves\" WHERE \"from\" = {v} OR \"to\" = {v} \
             ORDER BY \"space\"",
            v = sql_lit(version)
        ))
        .await
    {
        Ok(rows) => rows,
        Err(e) if e.is_missing_table() => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    if let Some(space) = moves
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .next()
    {
        return Err(refuse(
            format!("tenant {space} has a move open to or from"),
            &format!("Finish or revert it (`ordo evolve --tenant {space}`)"),
        ));
    }
    let v = Versions::on(&ops.pool);
    if v.own_pin().await?.is_some_and(|(_, on, _)| on == version) {
        return Err(refuse(
            "the deployment's own space is on".into(),
            "Move it with the rest (`ordo apply` or a rollout)",
        ));
    }
    Ok(())
}
