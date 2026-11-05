//! The deploy door's other three ops (`plan-console.md` 4.3 and 7, G1 to
//! G3), beside `std.deployment.apply`:
//!
//! - `std.deployment.versions` (G1): the stored versions, which one
//!   serves, and the activation log. What `ordo deploys` prints.
//! - `std.deployment.activate($version)` (G2): point the deployment at a
//!   stored version, which is a program rollback. What `ordo rollback`
//!   does.
//! - `std.deployment.plan($program)` (G3): what an apply of this program
//!   would do to the schema, with nothing stored.
//! - `std.deployment.label($version, $label?)`: set or clear a stored
//!   version's label. What `ordo versions label --host` does.
//! - `std.deployment.diff($a, $b)`: what changed between two stored
//!   versions. What `ordo versions diff --host` prints
//!   ([`crate::versions_diff`]).
//! - `std.deployment.revoke($version, $reason, $moveTo?)`: mark a stored
//!   version revoked, moving the deployment to `$moveTo` first when it is
//!   the active one (`plan-versions.md` §1.11). What `ordo versions revoke
//!   --host` does ([`apply_op::revoke`]).
//! - `std.deployment.unrevoke($version, $reason)`: lift the mark. What
//!   `ordo versions unrevoke --host` does.
//! - `std.deployment.revoked`: each revoked version and the tenants pinned
//!   to it. What `ordo versions revoked --host` prints.
//! - `std.deployment.moveOffRevoked($tenant)`: move one tenant off the
//!   revoked version it is on, to the target the app developer chose, now
//!   (`crate::revoked_moves`). What `ordo versions move --host` does.
//! - `std.deployment.setMoveTarget($tenant, $version?)`: set or clear where
//!   one tenant goes off a revoked version. What `ordo versions target
//!   --host` does.
//! - `std.deployment.versionSettings($limit?)`: `versions.served`,
//!   `versions.minimum`, `versions.new_tenant` and their history. What
//!   `ordo versions settings` prints.
//! - `std.deployment.setVersionSetting($setting, $value?, $reason)`:
//!   change one, or clear it (owner, 2026-11-05: they live in Ordo, not
//!   the environment). What `ordo versions set` does.
//!
//! The bodies are `crate::apply_op`'s ([`apply_op::history`],
//! [`apply_op::activate`], [`apply_op::preview`]), which the CLI calls
//! too, so the door and the box cannot drift. This module is names, specs,
//! the home-space check and the security record.
//!
//! # Who may call them
//!
//! The deployment owner, in the deployment's own space, on `APPLY`'s
//! three stacked checks: `owner_only` lists them, every change takes a
//! touch (`ORDO_STEP_UP_EXEMPT_OPS` may loosen that), and each answers
//! only when bound to the home space.
//!
//! `plan` is a `POST`, so the door counts it a change and asks for a touch.
//! Two reasons: a program is up to a megabyte, which a `GET` query cannot
//! carry, and the plan probes live rows across every space on the
//! deployment. The touch the console asks for covers the apply that
//! follows within 300 seconds.

use ordo_api::{
    BoundParams, Bounds, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use serde_json::json;

use crate::apply_op::{self, Activation, ApplyOps};

/// G1.
pub const VERSIONS: &str = "std.deployment.versions";
/// G2.
pub const ACTIVATE: &str = "std.deployment.activate";
/// G3.
pub const PLAN: &str = "std.deployment.plan";
/// A stored version's label (`plan-versions.md` §4 row 2).
pub const LABEL: &str = "std.deployment.label";
/// Two stored versions, compared.
pub const DIFF: &str = "std.deployment.diff";
/// Mark a stored version revoked (D8).
pub const REVOKE: &str = "std.deployment.revoke";
/// Lift a revoke mark (ask 20).
pub const UNREVOKE: &str = "std.deployment.unrevoke";
/// The revoked versions and the tenants pinned to them.
pub const REVOKED: &str = "std.deployment.revoked";
/// **Move one tenant to a stored version, now** (`plan-versions.md` §1.3,
/// increment 3): the move on demand (owner, 2026-11-05). A door op, so the
/// app developer's own system can drive moves as well as a person.
pub const PIN: &str = "std.deployment.pin";
/// One tenant's version, epoch and pin history.
pub const TENANT_VERSION: &str = "std.deployment.tenantVersion";
/// **Move one tenant off a revoked version, now** (ruled 2026-11-05): the
/// move on demand, by the deployment owner or the app developer's own
/// system, to the target the developer chose.
pub const MOVE_OFF_REVOKED: &str = "std.deployment.moveOffRevoked";
/// Where one tenant goes off a revoked version, set or cleared.
pub const SET_MOVE_TARGET: &str = "std.deployment.setMoveTarget";
/// **The three version settings and their history** (owner, 2026-11-05:
/// they live in Ordo, not the environment).
pub const VERSION_SETTINGS: &str = "std.deployment.versionSettings";
/// **Change one version setting**, or clear it, with a reason. A deployment
/// write, so it asks a touch only at `ORDO_SECURITY_LEVEL=high`
/// (`security_level::Situation::Deploying`); a machine key with the
/// `deployment` grant may call it.
pub const SET_VERSION_SETTING: &str = "std.deployment.setVersionSetting";

pub const OPS: [&str; 14] = [
    VERSIONS,
    ACTIVATE,
    PLAN,
    LABEL,
    DIFF,
    REVOKE,
    UNREVOKE,
    REVOKED,
    PIN,
    TENANT_VERSION,
    MOVE_OFF_REVOKED,
    SET_MOVE_TARGET,
    VERSION_SETTINGS,
    SET_VERSION_SETTING,
];

/// The ops that move tenants between versions and ask no passkey touch, of
/// a person or a system, strict deployment or not. The owner, 2026-11-05:
/// *"They could be moving many at a time, It would be ridiculous to ask for
/// each one. If they are logged in they proved their access."* A system
/// caller is never asked either (same day). The role gate is unchanged:
/// deployment owner only, on the deployment's own space, and a delegated
/// token stays refused (`delegable.rs`: platform writes are not delegable).
///
/// The moves off a revoked version and their target are moves too
/// (2026-11-05), and so is the tenant's own move when the app developer
/// grants it: the grant is the control.
pub const NO_PRESENCE: [&str; 4] = [
    PIN,
    MOVE_OFF_REVOKED,
    SET_MOVE_TARGET,
    crate::revoked_moves::MOVE_SELF,
];

/// The activation log's length when the caller names none.
pub const DEFAULT_LIMIT: i64 = 20;
/// The longest activation log one call returns.
pub const MAX_LIMIT: i64 = 200;

/// The deploy door's specs, apart from `APPLY`'s.
pub fn specs() -> Vec<OpSpec> {
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>, fields: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind,
        params,
        output_fields: fields.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(if kind == OpKind::Read {
            NotSyncable::NoAnchor
        } else {
            NotSyncable::NotARead
        }),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        spec(
            VERSIONS,
            OpKind::Read,
            vec![ParamSpec {
                name: "limit".to_string(),
                ty: ParamTy::Int,
                required: false,
                bounds: Some(Bounds {
                    min: Some("1".to_string()),
                    max: Some(MAX_LIMIT.to_string()),
                }),
            }],
            &["active", "stored", "activations"],
        ),
        spec(
            ACTIVATE,
            OpKind::Write,
            vec![ParamSpec {
                // A version hash; the registry's are well under this.
                name: "version".to_string(),
                ty: ParamTy::Text { max_len: Some(128) },
                required: true,
                bounds: None,
            }],
            &["state", "version", "from"],
        ),
        spec(
            PLAN,
            OpKind::Write,
            vec![ParamSpec {
                // The same bound `std.deployment.apply` takes, so whatever
                // can be applied can be previewed.
                name: "program".to_string(),
                ty: ParamTy::Text {
                    max_len: Some(1024 * 1024),
                },
                required: true,
                bounds: None,
            }],
            &[
                "version",
                "serving",
                "alreadyStored",
                "notes",
                "would",
                "changes",
            ],
        ),
        spec(
            LABEL,
            OpKind::Write,
            vec![
                ParamSpec {
                    // A version's name or `#n`.
                    name: "version".to_string(),
                    ty: ParamTy::Text { max_len: Some(128) },
                    required: true,
                    bounds: None,
                },
                ParamSpec {
                    // Absent clears the label. One line of at most 80
                    // characters, which the `Versions` client checks and
                    // names; this bound is only a ceiling.
                    name: "label".to_string(),
                    ty: ParamTy::Text {
                        max_len: Some(1024),
                    },
                    required: false,
                    bounds: None,
                },
            ],
            &["version", "ordinal", "label"],
        ),
        spec(
            DIFF,
            OpKind::Read,
            vec![
                version_param("a"),
                version_param("b"),
                // Each part is shown unless it is turned off: `ordo
                // versions diff --source` sends `schema=false`.
                flag_param("schema"),
                flag_param("source"),
            ],
            &["a", "b", "schema", "source"],
        ),
        spec(
            REVOKE,
            OpKind::Write,
            vec![
                version_param("version"),
                reason_param(),
                ParamSpec {
                    // Required by the `Versions` client when `version` is
                    // the active one, and refused when it is not.
                    name: "moveTo".to_string(),
                    ty: ParamTy::Text { max_len: Some(128) },
                    required: false,
                    bounds: None,
                },
            ],
            &["version", "ordinal", "revoked", "moved"],
        ),
        spec(
            UNREVOKE,
            OpKind::Write,
            vec![version_param("version"), reason_param()],
            &["version", "ordinal", "revoked"],
        ),
        spec(REVOKED, OpKind::Read, vec![], &["versions"]),
        spec(
            PIN,
            OpKind::Write,
            vec![
                ParamSpec {
                    name: "tenant".to_string(),
                    ty: ParamTy::Text { max_len: Some(128) },
                    required: true,
                    bounds: None,
                },
                version_param("version"),
            ],
            &["tenant", "from", "to", "epoch", "ordinal"],
        ),
        spec(
            TENANT_VERSION,
            OpKind::Read,
            vec![ParamSpec {
                name: "tenant".to_string(),
                ty: ParamTy::Text { max_len: Some(128) },
                required: true,
                bounds: None,
            }],
            &["tenant", "version", "ordinal", "label", "epoch", "history"],
        ),
        spec(
            MOVE_OFF_REVOKED,
            OpKind::Write,
            vec![tenant_param()],
            &["tenant", "from", "to", "epoch", "ordinal", "chosenBy"],
        ),
        spec(
            SET_MOVE_TARGET,
            OpKind::Write,
            vec![
                tenant_param(),
                ParamSpec {
                    // Absent clears the tenant's own target.
                    name: "version".to_string(),
                    ty: ParamTy::Text { max_len: Some(128) },
                    required: false,
                    bounds: None,
                },
            ],
            &["tenant", "target", "ordinal"],
        ),
        spec(
            VERSION_SETTINGS,
            OpKind::Read,
            vec![ParamSpec {
                name: "limit".to_string(),
                ty: ParamTy::Int,
                required: false,
                bounds: Some(Bounds {
                    min: Some("1".to_string()),
                    max: Some(crate::versions::MAX_SETTING_HISTORY.to_string()),
                }),
            }],
            &["served", "minimum", "newTenant", "history"],
        ),
        spec(
            SET_VERSION_SETTING,
            OpKind::Write,
            vec![
                ParamSpec {
                    // `versions.served`, `versions.minimum` or
                    // `versions.new_tenant`; the short names pass too.
                    name: "setting".to_string(),
                    ty: ParamTy::Text { max_len: Some(64) },
                    required: true,
                    bounds: None,
                },
                ParamSpec {
                    // Absent or empty clears it. Served takes `in-use` or
                    // a comma list of versions.
                    name: "value".to_string(),
                    ty: ParamTy::Text {
                        max_len: Some(4096),
                    },
                    required: false,
                    bounds: None,
                },
                reason_param(),
            ],
            &[
                "setting",
                "value",
                "changed",
                "served",
                "minimum",
                "newTenant",
            ],
        ),
    ]
}

/// The three settings, as both ops answer them. Unset reads as `null`.
fn settings_json(s: &crate::versions::VersionSettings) -> serde_json::Value {
    use crate::versions::VersionSetting;
    json!({
        "served": s.value(VersionSetting::Served),
        "minimum": s.value(VersionSetting::Minimum),
        "newTenant": s.value(VersionSetting::NewTenant),
    })
}

fn tenant_param() -> ParamSpec {
    ParamSpec {
        name: "tenant".to_string(),
        ty: ParamTy::Text { max_len: Some(128) },
        required: true,
        bounds: None,
    }
}

fn reason_param() -> ParamSpec {
    ParamSpec {
        // At most 500 characters, which the `Versions` client checks and
        // names; this bound is only a ceiling.
        name: "reason".to_string(),
        ty: ParamTy::Text {
            max_len: Some(4096),
        },
        required: true,
        bounds: None,
    }
}

fn version_param(name: &str) -> ParamSpec {
    ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(128) },
        required: true,
        bounds: None,
    }
}

fn flag_param(name: &str) -> ParamSpec {
    ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Bool,
        required: false,
        bounds: None,
    }
}

/// Run one of them. `caller` is the principal the door proved.
pub async fn execute(
    ops: &ApplyOps,
    name: &str,
    space: &str,
    bound: &BoundParams,
    caller: &str,
) -> Result<ExecResult, ExecError> {
    ops.home_only(space)?;
    let row = match name {
        VERSIONS => {
            let limit = bound
                .0
                .get("limit")
                .and_then(|v| v.as_i64())
                .unwrap_or(DEFAULT_LIMIT)
                .clamp(1, MAX_LIMIT);
            apply_op::history(&ops.pool, usize::try_from(limit).unwrap_or(20))
                .await
                .map_err(|e| e.to_exec_error("the deployment's program history could not be read"))?
                .to_json()
        }
        ACTIVATE => {
            let version = bound
                .0
                .get("version")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or_default();
            // A version is its name or its order number, `#4`, as for
            // `ordo versions` and `ordo rollback`. A name goes as it is.
            let version = if version.starts_with('#') {
                match crate::versions::Versions::on(&ops.pool)
                    .resolve(version)
                    .await
                {
                    Ok(v) => v,
                    Err(e) => return Err(e.to_exec_error("the version could not be activated")),
                }
            } else {
                version.to_string()
            };
            let version = version.as_str();
            match apply_op::activate(&ops.pool, version, caller).await {
                Ok(Activation::Switched { from, to }) => {
                    ops.record(
                        caller,
                        ACTIVATE,
                        "activated",
                        format!(
                            "version={to} was={} by={caller}",
                            from.as_deref().unwrap_or("none")
                        ),
                    );
                    json!({ "state": "switched", "version": to, "from": from })
                }
                Ok(Activation::AlreadyActive { version }) => {
                    json!({ "state": "alreadyActive", "version": version, "from": version })
                }
                Ok(Activation::Staged) => {
                    return Err(ExecError::internal("an activation answered staged"))
                }
                Err(refusal) => return Err(ExecError::new(refusal.code(), refusal.to_string())),
            }
        }
        PLAN => {
            let source = bound
                .0
                .get("program")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ExecError::internal("`$program` did not reach the platform op"))?;
            apply_op::preview(&ops.pool, source)
                .await
                .map_err(|refusal| ExecError::new(refusal.code(), refusal.to_string()))?
                .to_json()
        }
        LABEL => {
            let version = text(bound, "version").unwrap_or_default();
            let label = text(bound, "label");
            let v = crate::versions::Versions::on(&ops.pool);
            let version = v
                .set_label(version, label)
                .await
                .map_err(|e| e.to_exec_error("the version's label was not changed"))?;
            ops.record(
                caller,
                LABEL,
                if label.is_some() {
                    "labelled"
                } else {
                    "unlabelled"
                },
                format!("version={version} by={caller}"),
            );
            let ordinal = v
                .ordinal_of(&version)
                .await
                .map_err(|e| e.to_exec_error("the version's order number could not be read"))?;
            let label = v
                .provenance(&version)
                .await
                .map_err(|e| e.to_exec_error("the version's label could not be read"))?
                .label;
            json!({ "version": version, "ordinal": ordinal, "label": label })
        }
        DIFF => {
            let on = |k: &str| bound.0.get(k).and_then(|v| v.as_bool()).unwrap_or(true);
            crate::versions_diff::diff(
                &ops.pool,
                text(bound, "a").unwrap_or_default(),
                text(bound, "b").unwrap_or_default(),
                on("schema"),
                on("source"),
            )
            .await
            .map_err(|e| e.to_exec_error("the two versions could not be compared"))?
            .to_json()
        }
        REVOKE => {
            let version = text(bound, "version").unwrap_or_default();
            let reason = text(bound, "reason").unwrap_or_default();
            let move_to = text(bound, "moveTo").filter(|m| !m.is_empty());
            let r = apply_op::revoke(&ops.pool, version, reason, move_to, caller)
                .await
                .map_err(|e| e.to_exec_error("the version was not revoked"))?;
            if let Some((from, to)) = &r.moved {
                ops.record(
                    caller,
                    REVOKE,
                    "activated",
                    format!("version={to} was={from} by={caller}"),
                );
            }
            ops.record(
                caller,
                REVOKE,
                "revoked",
                format!("version={} by={caller}", r.version),
            );
            revoked_row(ops, &r.version, r.moved.as_ref().map(|(_, to)| to.as_str())).await?
        }
        UNREVOKE => {
            let version = text(bound, "version").unwrap_or_default();
            let reason = text(bound, "reason").unwrap_or_default();
            let version = crate::versions::Versions::on(&ops.pool)
                .unrevoke(version, reason, caller, crate::runtime::now_secs())
                .await
                .map_err(|e| e.to_exec_error("the revoke mark was not lifted"))?;
            ops.record(
                caller,
                UNREVOKE,
                "unrevoked",
                format!("version={version} by={caller}"),
            );
            let mut row = revoked_row(ops, &version, None).await?;
            if let Some(o) = row.as_object_mut() {
                o.remove("moved");
            }
            row
        }
        PIN => {
            let tenant = text(bound, "tenant").unwrap_or_default();
            let version = text(bound, "version").unwrap_or_default();
            let v = crate::versions::Versions::on(&ops.pool);
            let moved = v
                .pin(tenant, version, caller, crate::runtime::now_secs())
                .await
                .map_err(|e| e.to_exec_error("the tenant was not moved"))?;
            ops.record(
                caller,
                PIN,
                "pinned",
                format!(
                    "tenant={tenant} version={} was={} by={caller}",
                    moved.to,
                    moved.from.as_deref().unwrap_or("none")
                ),
            );
            let ordinal = v
                .ordinal_of(&moved.to)
                .await
                .map_err(|e| e.to_exec_error("the version's order number could not be read"))?;
            json!({
                "tenant": moved.space,
                "from": moved.from,
                "to": moved.to,
                "epoch": moved.epoch,
                "ordinal": ordinal,
            })
        }
        TENANT_VERSION => {
            let tenant = text(bound, "tenant").unwrap_or_default();
            let v = crate::versions::Versions::on(&ops.pool);
            let read = |e: crate::versions::VersionRefusal| {
                e.to_exec_error("the tenant's pin could not be read")
            };
            let Some(pin) = v.pin_of(tenant).await.map_err(read)? else {
                return Err(crate::versions::VersionRefusal::NoTenant {
                    space: tenant.to_string(),
                }
                .to_exec_error("the tenant's pin could not be read"));
            };
            let (ordinal, label) = match &pin.version {
                Some(version) => (
                    v.ordinal_of(version).await.map_err(read)?,
                    v.provenance(version).await.map_err(read)?.label,
                ),
                None => (None, None),
            };
            let history = v.pin_history(tenant).await.map_err(read)?;
            json!({
                "tenant": tenant,
                "version": pin.version,
                "ordinal": ordinal,
                "label": label,
                "epoch": pin.epoch,
                "history": history.iter().map(|c| json!({
                    "from": c.from, "to": c.to, "epoch": c.epoch,
                    "actor": c.actor, "cause": c.cause, "at": c.at,
                })).collect::<Vec<_>>(),
            })
        }
        REVOKED => {
            let mut list = crate::versions::Versions::on(&ops.pool)
                .pinned_to_revoked()
                .await
                .map_err(|e| e.to_exec_error("the revoked versions could not be read"))?;
            crate::revoked_moves::note_next_windows(
                &ops.pool,
                &mut list,
                crate::runtime::now_secs(),
            )
            .await;
            json!({ "versions": list.iter().map(revoked_json).collect::<Vec<_>>() })
        }
        MOVE_OFF_REVOKED => {
            let tenant = text(bound, "tenant").unwrap_or_default();
            let moved = crate::versions::Versions::on(&ops.pool)
                .move_off_revoked(
                    tenant,
                    caller,
                    crate::versions::PinCause::Demand,
                    crate::runtime::now_secs(),
                )
                .await
                .map_err(|e| e.to_exec_error("the tenant was not moved"))?;
            ops.record(
                caller,
                MOVE_OFF_REVOKED,
                "moved-off-revoked",
                format!(
                    "tenant={tenant} version={} was={} by={caller}",
                    moved.change.to,
                    moved.change.from.as_deref().unwrap_or("none")
                ),
            );
            crate::revoked_moves::moved_json(&ops.pool, &moved).await?
        }
        SET_MOVE_TARGET => {
            let tenant = text(bound, "tenant").unwrap_or_default();
            let version = text(bound, "version").filter(|v| !v.is_empty());
            let v = crate::versions::Versions::on(&ops.pool);
            let set = v
                .set_move_target(tenant, version, caller, crate::runtime::now_secs())
                .await
                .map_err(|e| e.to_exec_error("the tenant's move target was not set"))?;
            ops.record(
                caller,
                SET_MOVE_TARGET,
                "move-target",
                format!(
                    "tenant={tenant} target={} by={caller}",
                    set.as_deref().unwrap_or("cleared")
                ),
            );
            let ordinal = match &set {
                Some(t) => v
                    .ordinal_of(t)
                    .await
                    .map_err(|e| e.to_exec_error("the version's order number could not be read"))?,
                None => None,
            };
            json!({ "tenant": tenant, "target": set, "ordinal": ordinal })
        }
        VERSION_SETTINGS => {
            let limit = bound
                .0
                .get("limit")
                .and_then(|v| v.as_i64())
                .unwrap_or(DEFAULT_LIMIT)
                .clamp(1, crate::versions::MAX_SETTING_HISTORY as i64);
            let v = crate::versions::Versions::on(&ops.pool);
            let read = |e: crate::versions::VersionRefusal| {
                e.to_exec_error("the version settings could not be read")
            };
            let settings = v.settings().await.map_err(read)?;
            let history = v
                .setting_history(usize::try_from(limit).unwrap_or(20))
                .await
                .map_err(read)?;
            let mut row = settings_json(&settings);
            row["history"] = json!(history
                .iter()
                .map(|c| json!({
                    "setting": c.setting, "value": c.value, "actor": c.actor,
                    "reason": c.reason, "at": c.at,
                }))
                .collect::<Vec<_>>());
            row
        }
        SET_VERSION_SETTING => {
            let named = text(bound, "setting").unwrap_or_default();
            let Some(setting) = crate::versions::VersionSetting::parse(named) else {
                return Err(ExecError::new(
                    ordo_api::ErrorCode::InvalidParam,
                    format!(
                        "the version setting was not changed: `{named}` is not one. There are \
                         versions.served, versions.minimum and versions.new_tenant"
                    ),
                ));
            };
            let value = text(bound, "value").filter(|v| !v.is_empty());
            let reason = text(bound, "reason").unwrap_or_default();
            let set = crate::versions::Versions::on(&ops.pool)
                .set_setting(setting, value, caller, reason, crate::runtime::now_secs())
                .await
                .map_err(|e| e.to_exec_error("the version setting was not changed"))?;
            let changed = matches!(set, crate::versions::SettingSet::Changed(_));
            let now = set.settings().value(setting);
            if changed {
                ops.record(
                    caller,
                    SET_VERSION_SETTING,
                    "version-setting",
                    format!(
                        "setting={} value={} by={caller}",
                        setting.name(),
                        now.as_deref().unwrap_or("unset")
                    ),
                );
            }
            let mut row = settings_json(set.settings());
            row["setting"] = json!(setting.name());
            row["value"] = json!(now);
            row["changed"] = json!(changed);
            row
        }
        other => return Err(ExecError::internal(format!("`{other}` is not a deploy op"))),
    };
    Ok(ExecResult {
        position: None,
        rows: vec![row],
        next_cursor: None,
    })
}

/// What `revoke` and `unrevoke` answer: the version, its order number,
/// its mark now (`null` when lifted) and where the deployment moved.
async fn revoked_row(
    ops: &ApplyOps,
    version: &str,
    moved: Option<&str>,
) -> Result<serde_json::Value, ExecError> {
    let v = crate::versions::Versions::on(&ops.pool);
    let read =
        |e: crate::versions::VersionRefusal| e.to_exec_error("the version could not be read");
    let ordinal = v.ordinal_of(version).await.map_err(read)?;
    let mark = v.revocation_of(version).await.map_err(read)?;
    Ok(json!({
        "version": version,
        "ordinal": ordinal,
        "revoked": mark.map(|m| json!({ "reason": m.reason, "actor": m.actor, "at": m.at })),
        "moved": moved,
    }))
}

/// One entry of `std.deployment.revoked`.
pub fn revoked_json(r: &crate::versions::RevokedVersion) -> serde_json::Value {
    json!({
        "version": r.mark.version,
        "ordinal": r.ordinal,
        "reason": r.mark.reason,
        "actor": r.mark.actor,
        "at": r.mark.at,
        "moveTo": r.mark.move_to,
        "tenantCount": r.tenant_count,
        "tenants": r.tenants.iter().map(|t| json!({
            "space": t.space,
            "name": t.name,
            "state": t.state,
            "target": t.target,
            "nextWindow": t.next_window,
        })).collect::<Vec<_>>(),
    })
}

/// [`revoked_json`] read back, so `ordo versions revoked --host` prints
/// what the direct path prints.
pub fn revoked_from_json(v: &serde_json::Value) -> Option<crate::versions::RevokedVersion> {
    use crate::versions::{Revocation, RevokeAct, RevokedPin, RevokedVersion};
    Some(RevokedVersion {
        mark: Revocation {
            seq: 0,
            version: v["version"].as_str()?.to_string(),
            act: RevokeAct::Revoke,
            actor: v["actor"].as_str()?.to_string(),
            reason: v["reason"].as_str()?.to_string(),
            at: v["at"].as_u64()?,
            move_to: v["moveTo"].as_str().map(str::to_string),
        },
        ordinal: v["ordinal"].as_u64(),
        tenants: v["tenants"]
            .as_array()?
            .iter()
            .filter_map(|t| {
                Some(RevokedPin {
                    space: t["space"].as_str()?.to_string(),
                    name: t["name"].as_str()?.to_string(),
                    state: t["state"].as_str()?.to_string(),
                    target: t["target"].as_str().map(str::to_string),
                    next_window: t["nextWindow"].as_i64(),
                })
            })
            .collect(),
        tenant_count: v["tenantCount"].as_u64()?,
    })
}

/// A text parameter, trimmed.
fn text<'b>(bound: &'b BoundParams, k: &str) -> Option<&'b str> {
    bound.0.get(k).and_then(|v| v.as_str()).map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_api::ErrorCode;

    #[test]
    fn the_read_is_a_read_and_the_two_that_post_are_changes() {
        let specs = specs();
        let kind = |n: &str| specs.iter().find(|s| s.name == n).map(|s| s.kind);
        assert_eq!(kind(VERSIONS), Some(OpKind::Read));
        assert_eq!(kind(ACTIVATE), Some(OpKind::Write));
        assert_eq!(kind(PLAN), Some(OpKind::Write));
        assert_eq!(kind(LABEL), Some(OpKind::Write));
        assert_eq!(kind(DIFF), Some(OpKind::Read));
        assert_eq!(kind(REVOKE), Some(OpKind::Write));
        assert_eq!(kind(UNREVOKE), Some(OpKind::Write));
        assert_eq!(kind(REVOKED), Some(OpKind::Read));
        assert_eq!(specs.len(), OPS.len());
    }

    #[test]
    fn an_unknown_version_is_not_found_and_names_where_the_versions_are() {
        let r = apply_op::ApplyRefused::UnknownVersion("v9".into());
        assert_eq!(r.code(), ErrorCode::NotFound);
        assert_eq!(r.class(), "unknown_version");
        assert!(r.to_string().contains("std.deployment.versions"));
    }
}
