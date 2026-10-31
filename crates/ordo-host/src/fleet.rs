//! The deployment's tenants, in one place: `ordo fleet` and
//! `std.deployment.tenants` (G4, `plan-console.md` 4.4 and 7).
//!
//! # One reading, two surfaces
//!
//! `ordo fleet` opens the control database and prints each database target
//! with its version, its custody and the tenants on it. The console cannot
//! open that database (`plan-console.md` 3): it reaches the deployment only
//! through the door. So the reading lives here, and both surfaces call it:
//!
//! - [`read`] answers the targets, each with its tenants. The CLI renders it.
//! - [`TenantListOps`] answers `std.deployment.tenants`: one row per tenant,
//!   with the target it is on from the same [`read`], and what only a
//!   running deployment knows beside it.
//!
//! # What a row says (ask 5, ruled 2026-10-23)
//!
//! The deployment owner sees every tenant's id, name, status, counts and
//! usage, and when it was made. Never its members or its shares: the
//! member count is a number, read from the door's own directory, and no
//! principal is named. `yourRole` is the role the caller holds in that
//! tenant, if any, which is what tells the console where the tenant's own
//! ops (export, delete, a bucket) can be offered.
//!
//! # Per node
//!
//! `members` and `quarantined` are this node's: the directory and the
//! quarantine register live in each replica's memory. The rest is read from
//! the control database and is the same on every node.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_crypto::custody::SpaceId;
use ordo_exec::spacedb::{DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolError};
use ordo_identity::principal::{MembershipStatus, PrincipalId, Role};
use ordo_identity::AccessEngine;
use serde_json::{json, Value};

use crate::tenants::{self, State, Tenant};

/// `std.deployment.tenants($includeReaped?)`.
pub const TENANTS: &str = "std.deployment.tenants";
/// Every op this module serves.
pub const OPS: [&str; 1] = [TENANTS];

/// One database target and the tenants on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// `default`, or `host:port/db`.
    pub label: String,
    /// The version the target last reported, and when. `None`: not yet
    /// confirmed.
    pub confirmed: Option<ordo_exec::fleet::Confirmed>,
    /// `None` for `default`, the deployment's own database.
    pub custody: Option<DbCustody>,
    /// Each tenant on it: id and name.
    pub tenants: Vec<(String, String)>,
}

/// What the control database says about the fleet: every target, `default`
/// first, and every registered tenant.
#[derive(Debug, Clone)]
pub struct Fleet {
    pub targets: Vec<Target>,
    /// Every tenant in the registry, newest first.
    pub tenants: Vec<Tenant>,
}

impl Fleet {
    /// The target a tenant is on, and its custody.
    pub fn target_of(&self, id: &str) -> Option<&Target> {
        self.targets
            .iter()
            .find(|t| t.tenants.iter().any(|(tid, _)| tid == id))
    }
}

/// Make the tables [`read`] reads, if they are missing. `ordo fleet` runs
/// it first, as it always has; a running deployment made them at boot.
pub async fn ensure_tables(pool: &Pool) -> Result<(), PoolError> {
    ordo_exec::fleet::ensure_table(pool).await?;
    ordo_exec::spacedb::ensure_table(pool).await?;
    tenants::ensure_table(pool).await
}

/// Read the fleet from the control database.
pub async fn read(pool: &Pool) -> Result<Fleet, PoolError> {
    let confirmed = ordo_exec::fleet::all(pool).await?;
    let attachments = ordo_exec::spacedb::all(pool).await?;
    let tenants = tenants::all(pool).await?;
    Ok(Fleet {
        targets: group(&confirmed, &attachments, &tenants),
        tenants,
    })
}

/// One target per distinct database, `default` first, then every attached
/// target, each with its tenants. A pure function of the three reads.
pub fn group(
    confirmed: &[ordo_exec::fleet::Confirmed],
    attachments: &[(SpaceId, DbAttachment)],
    tenants: &[Tenant],
) -> Vec<Target> {
    let name_of = |id: &str| -> String {
        tenants
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "(unnamed)".to_string())
    };
    let mut targets: Vec<Target> = vec![Target {
        label: ordo_exec::route::DEFAULT_LABEL.to_string(),
        confirmed: None,
        custody: None,
        tenants: Vec::new(),
    }];
    for (space, a) in attachments {
        let label = ordo_exec::route::target_label(&a.host, a.port, &a.db);
        let idx = match targets.iter().position(|t| t.label == label) {
            Some(i) => i,
            None => {
                targets.push(Target {
                    label: label.clone(),
                    confirmed: None,
                    custody: None,
                    tenants: Vec::new(),
                });
                targets.len() - 1
            }
        };
        targets[idx].custody = Some(a.custody);
        targets[idx]
            .tenants
            .push((space.0.clone(), name_of(&space.0)));
    }
    // A tenant with no `$spacedb` row is on `default`: a space is on the
    // deployment's own database until something attaches it (§9.1).
    let attached: BTreeSet<&str> = attachments.iter().map(|(s, _)| s.0.as_str()).collect();
    for t in tenants {
        if !attached.contains(t.id.as_str()) {
            targets[0].tenants.push((t.id.clone(), t.name.clone()));
        }
    }
    for t in &mut targets {
        t.confirmed = confirmed.iter().find(|c| c.label == t.label).cloned();
    }
    targets
}

/// The latest rolled-up usage period of every space that has one
/// (`$usage`, `ordo_exec::usage`).
async fn latest_usage(pool: &Pool) -> Result<BTreeMap<String, Value>, PoolError> {
    let rows = pool
        .rows(
            "SELECT DISTINCT ON (\"space\") \"space\", \"period_start\", \"period_end\", \
             \"writes\", \"rows_written\", \"bytes_sealed\", \"bytes_chain\", \"bytes_blob\", \
             \"bytes_rows_estimated\" \
             FROM \"$usage\" ORDER BY \"space\", \"period_start\" DESC",
        )
        .await?;
    let num = |r: &[Option<String>], i: usize| -> u64 {
        r.get(i)
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    };
    Ok(rows
        .iter()
        .filter_map(|r| {
            let space = r.first()?.clone()?;
            Some((
                space,
                json!({
                    "periodStart": num(r, 1),
                    "periodEnd": num(r, 2),
                    "writes": num(r, 3),
                    "rowsWritten": num(r, 4),
                    "bytesSealed": num(r, 5),
                    "bytesChain": num(r, 6),
                    "bytesBlob": num(r, 7),
                    "bytesRowsEstimated": num(r, 8),
                }),
            ))
        })
        .collect())
}

fn role_word(role: Option<Role>) -> Option<&'static str> {
    match role {
        Some(Role::Owner) => Some("owner"),
        Some(Role::Manager) => Some("manager"),
        Some(Role::Editor) => Some("editor"),
        Some(Role::Viewer) => Some("viewer"),
        // A guest seat holds no role.
        None => None,
    }
}

/// The op's spec: a read, with one optional flag.
pub fn specs() -> Vec<OpSpec> {
    vec![OpSpec {
        name: TENANTS.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind: OpKind::Read,
        params: vec![ParamSpec {
            // Reaped tenants are history: left out unless asked for.
            name: "includeReaped".to_string(),
            ty: ParamTy::Bool,
            required: false,
            bounds: None,
        }],
        output_fields: [
            "id",
            "name",
            "status",
            "phase",
            "createdAt",
            "stateAt",
            "own",
            "note",
            "pausedAt",
            "pausedBy",
            "pausedNote",
            "target",
            "custody",
            "members",
            "usage",
            "quarantined",
            "yourRole",
        ]
        .iter()
        .map(|f| FieldSpec::new(*f))
        .collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }]
}

/// What the op needs.
pub struct TenantListOps {
    /// The control database: the registry, `$spacedb`, `$fleet`, `$usage`.
    pub control: Pool,
    /// The deployment's own space, the only one the op answers in.
    pub home: String,
    /// This node's quarantine register. `None`: no resolver, nothing held.
    pub spaces: Option<Arc<crate::spaces::Spaces>>,
}

impl TenantListOps {
    /// `std.deployment.tenants`: one row per tenant, newest first.
    pub async fn execute(
        &self,
        engine: &AccessEngine,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if space != self.home {
            // `not_found`, as every deployment op answers outside its home.
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "the list of tenants is the deployment's: this op answers only in the \
                 deployment's own space",
            ));
        }
        let unreadable = |e: PoolError| {
            ExecError::of_fault(
                e.class(),
                ErrorCode::SpaceUnavailable,
                format!(
                    "the tenant registry could not be read ({e}). Nothing was read, which is \
                     not the same as there being no tenants"
                ),
            )
        };
        let fleet = read(&self.control).await.map_err(unreadable)?;
        let usage = latest_usage(&self.control).await.map_err(unreadable)?;
        let include_reaped = bound
            .0
            .get("includeReaped")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let quarantined: BTreeMap<String, String> = self
            .spaces
            .as_deref()
            .map(|s| s.quarantined().into_iter().collect())
            .unwrap_or_default();
        // The caller is the member in the home space; the roles it holds
        // elsewhere are the person's.
        let person = engine
            .principal_of_member(&ordo_identity::SpaceId(space.to_string()), caller)
            .cloned()
            .unwrap_or_else(|| PrincipalId(caller.to_string()));
        let roles: BTreeMap<String, Option<&'static str>> = engine
            .spaces_of(&person)
            .into_iter()
            .filter(|(_, _, status)| *status == MembershipStatus::Active)
            .map(|(s, role, _)| (s.0, role_word(role)))
            .collect();

        let rows = fleet
            .tenants
            .iter()
            .filter(|t| include_reaped || t.state != State::Reaped)
            .map(|t| {
                let target = fleet.target_of(&t.id);
                let serving = matches!(t.state, State::Active | State::Paused);
                let paused = t.state == State::Paused;
                json!({
                    "id": t.id,
                    "name": t.name,
                    "status": t.state.as_str(),
                    "phase": t.phase.as_str(),
                    "createdAt": t.created_at,
                    "stateAt": t.state_at,
                    "own": t.own_of.is_some(),
                    // Why it failed. Empty unless it did.
                    "note": if t.state == State::Failed { json!(t.note) } else { Value::Null },
                    "pausedAt": if paused { json!(t.paused_at) } else { Value::Null },
                    "pausedBy": if paused { json!(t.paused_by) } else { Value::Null },
                    "pausedNote": if paused { json!(t.paused_note) } else { Value::Null },
                    "target": target.map(|x| x.label.clone()),
                    "custody": target.map(|x| {
                        x.custody.map(|c| c.as_str()).unwrap_or("platform")
                    }),
                    "members": if serving {
                        json!(engine.members_of(&ordo_identity::SpaceId(t.id.clone())).len())
                    } else {
                        Value::Null
                    },
                    "usage": usage.get(&t.id).cloned().unwrap_or(Value::Null),
                    "quarantined": quarantined.get(&t.id),
                    "yourRole": roles.get(&t.id).copied().flatten(),
                })
            })
            .collect();
        Ok(ExecResult {
            position: None,
            rows,
            next_cursor: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(id: &str, name: &str) -> Tenant {
        Tenant {
            id: id.into(),
            name: name.into(),
            created_at: 1,
            state: State::Active,
            phase: tenants::Phase::Founded,
            target: String::new(),
            state_at: 1,
            note: String::new(),
            created_by: "x".into(),
            own_of: None,
            paused_at: 0,
            paused_by: String::new(),
            paused_note: String::new(),
            lease_until: 0,
            attempts: 0,
            retry_at: 0,
            stuck_at: 0,
        }
    }

    #[test]
    fn an_unattached_tenant_is_on_default_and_every_target_keeps_its_version() {
        let confirmed = vec![ordo_exec::fleet::Confirmed {
            label: "default".into(),
            version: "7".into(),
            confirmed_at: 9,
        }];
        let targets = group(&confirmed, &[], &[tenant("spc_a", "Acme")]);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].label, "default");
        assert_eq!(targets[0].tenants, vec![("spc_a".into(), "Acme".into())]);
        assert_eq!(targets[0].confirmed.as_ref().unwrap().version, "7");
    }
}
