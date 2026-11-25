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
//! - `std.deployment.environments($name?)`: the declared chain of
//!   environments and which one this deployment is; with `$name`, that one
//!   or a refusal naming the command that registers it. What `ordo
//!   environments list` prints.
//! - `std.deployment.registerEnvironment($name, $position?, $reason)`: add
//!   one to the chain (`plan-versions.md` §4 row 4a). What `ordo
//!   environments register` does.
//! - `std.deployment.setOwnEnvironment($name, $reason)`, `removeEnvironment`
//!   and `renameEnvironment($name, $to, $reason)`: which environment this
//!   deployment is, and removing or renaming one. Each is recorded.
//! - `std.deployment.setEnvironmentDoor($name, $address?, $space?,
//!   $handle?, $reason)`: where an environment's deployment answers, or
//!   clear it. What `ordo environments door` does.
//! - `std.deployment.setPromotionSetting($setting, $value, $reason)`:
//!   `promotion.skip_staging`. What `ordo environments set` does.
//! - `std.deployment.versionSource($version)`: a stored version as a
//!   promotion's source, with its identity, whether it was activated and
//!   who read it. What the next step's gate reads from this one.
//! - `std.deployment.promote($version, $fromToken, $reason, $activate?)`:
//!   the promotion gate on this deployment ([`crate::promotion`]). It reads
//!   the version from the step before itself, with the caller's own token
//!   for it. What `ordo promote` does.
//! - `std.deployment.evolve`, `std.deployment.plans` and
//!   `std.deployment.replay`: stored plans ([`crate::evolve`]). What `ordo
//!   evolve` does.
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

/// **The chain of environments** and which one this deployment is
/// (`plan-versions.md` §3.1). With a name, a command that names an
/// environment: refused when it is not registered.
pub const ENVIRONMENTS: &str = "std.deployment.environments";
/// **Register an environment** in the chain, with a reason.
pub const REGISTER_ENVIRONMENT: &str = "std.deployment.registerEnvironment";

/// Say which environment this deployment is.
pub const SET_OWN_ENVIRONMENT: &str = "std.deployment.setOwnEnvironment";
/// Remove an environment from the chain.
pub const REMOVE_ENVIRONMENT: &str = "std.deployment.removeEnvironment";
/// Rename an environment in the chain.
pub const RENAME_ENVIRONMENT: &str = "std.deployment.renameEnvironment";
/// **A stored version as a promotion's source.**
pub const VERSION_SOURCE: &str = "std.deployment.versionSource";
/// **Promote a version into this deployment**, through the gate.
pub const PROMOTE: &str = "std.deployment.promote";
/// Where an environment's deployment answers.
pub const SET_ENVIRONMENT_DOOR: &str = "std.deployment.setEnvironmentDoor";
/// Set a promotion setting (`promotion.skip_staging`).
pub const SET_PROMOTION_SETTING: &str = "std.deployment.setPromotionSetting";

/// **Render, check and store a plan** from one stored version to another
/// (`plan-versions.md` §2.3), and run it on the deployment's own database
/// when asked. What `ordo evolve --from --to` does.
pub const EVOLVE: &str = crate::evolve::EVOLVE;
/// The stored plans, or one edge's.
pub const PLANS: &str = crate::evolve::PLANS;
/// Replay the stored path on one tenant's database, then pin it.
pub const REPLAY: &str = crate::evolve::REPLAY;
/// What a rollback to a version would find: its externals and peers now.
pub const ROLLBACK_CHECK: &str = crate::evolve::ROLLBACK_CHECK;
/// A tenant in its soak back on the version it moved from (increment 11a).
pub const REVERT_SOAK: &str = crate::evolve::REVERT_SOAK;
/// End a tenant's soak now (increment 11a).
pub const CLOSE_SOAK: &str = crate::evolve::CLOSE_SOAK;
/// **Rollouts** (`plan-versions.md` §3.7, increment 10): start one, read
/// where it stands, resume, skip a tenant, retry one, abort, finish by
/// hand, and hold a tenant ([`crate::rollouts`]). What `ordo rollout` does.
pub const ROLLOUT_START: &str = crate::rollouts::START;
pub const ROLLOUT_STATUS: &str = crate::rollouts::STATUS;
pub const ROLLOUT_RESUME: &str = crate::rollouts::RESUME;
pub const ROLLOUT_SKIP: &str = crate::rollouts::SKIP;
pub const ROLLOUT_RETRY: &str = crate::rollouts::RETRY;
pub const ROLLOUT_ABORT: &str = crate::rollouts::ABORT;
pub const ROLLOUT_FINISH: &str = crate::rollouts::FINISH;
pub const ROLLOUT_HOLD: &str = crate::rollouts::HOLD;

/// What versions left behind in every database (`$orphans`).
pub const ORPHANS: &str = crate::orphan_ops::ORPHANS;
/// Remove one orphan from every database, once asked twice.
pub const PRUNE: &str = crate::orphan_ops::PRUNE;

pub const OPS: [&str; 39] = [
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
    ENVIRONMENTS,
    REGISTER_ENVIRONMENT,
    SET_OWN_ENVIRONMENT,
    REMOVE_ENVIRONMENT,
    RENAME_ENVIRONMENT,
    VERSION_SOURCE,
    PROMOTE,
    SET_ENVIRONMENT_DOOR,
    SET_PROMOTION_SETTING,
    EVOLVE,
    PLANS,
    REPLAY,
    ROLLBACK_CHECK,
    REVERT_SOAK,
    CLOSE_SOAK,
    ROLLOUT_START,
    ROLLOUT_STATUS,
    ROLLOUT_RESUME,
    ROLLOUT_SKIP,
    ROLLOUT_RETRY,
    ROLLOUT_ABORT,
    ROLLOUT_FINISH,
    ROLLOUT_HOLD,
    ORPHANS,
    PRUNE,
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
///
/// A rollout's controls are moves too: no reauth (§3.7, owner 2026-11-05).
pub const NO_PRESENCE: [&str; 11] = [
    PIN,
    MOVE_OFF_REVOKED,
    SET_MOVE_TARGET,
    crate::revoked_moves::MOVE_SELF,
    ROLLOUT_START,
    ROLLOUT_RESUME,
    ROLLOUT_SKIP,
    ROLLOUT_RETRY,
    ROLLOUT_ABORT,
    ROLLOUT_FINISH,
    ROLLOUT_HOLD,
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
        reads_external: false,
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
            &["state", "version", "from", "reverted"],
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
            &[
                "served",
                "minimum",
                "newTenant",
                "minResidency",
                "idleEvictAfter",
                "history",
                "waiting",
            ],
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
                "minResidency",
                "idleEvictAfter",
                "waiting",
            ],
        ),
        spec(
            ENVIRONMENTS,
            OpKind::Read,
            vec![ParamSpec {
                name: "name".to_string(),
                ty: ParamTy::Text { max_len: Some(128) },
                required: false,
                bounds: None,
            }],
            &["own", "chain", "environment", "skipStaging"],
        ),
        spec(
            REGISTER_ENVIRONMENT,
            OpKind::Write,
            vec![
                ParamSpec {
                    name: "name".to_string(),
                    ty: ParamTy::Text { max_len: Some(128) },
                    required: true,
                    bounds: None,
                },
                ParamSpec {
                    // 1 is the first step. Absent puts it before this
                    // deployment's own.
                    name: "position".to_string(),
                    ty: ParamTy::Int,
                    required: false,
                    bounds: Some(Bounds {
                        min: Some("1".to_string()),
                        max: Some("64".to_string()),
                    }),
                },
                optional_text("address", 256),
                optional_text("space", 64),
                optional_text("handle", 63),
                reason_param(),
            ],
            &["registered", "own", "chain"],
        ),
        spec(
            SET_OWN_ENVIRONMENT,
            OpKind::Write,
            vec![name_param(), reason_param()],
            &["own", "changed", "chain"],
        ),
        spec(
            REMOVE_ENVIRONMENT,
            OpKind::Write,
            vec![name_param(), reason_param()],
            &["removed", "own", "chain"],
        ),
        spec(
            RENAME_ENVIRONMENT,
            OpKind::Write,
            vec![
                name_param(),
                ParamSpec {
                    name: "to".to_string(),
                    ty: ParamTy::Text { max_len: Some(128) },
                    required: true,
                    bounds: None,
                },
                reason_param(),
            ],
            &["renamed", "to", "own", "chain"],
        ),
        spec(
            VERSION_SOURCE,
            OpKind::Read,
            vec![version_param("version")],
            &[
                "env",
                "version",
                "source",
                "sourceId",
                "codeHashes",
                "code",
                "activated",
                "readBy",
            ],
        ),
        spec(
            PROMOTE,
            OpKind::Write,
            vec![
                version_param("version"),
                ParamSpec {
                    // The caller's own token for the step before, from the
                    // same sign-in. Sent there once, never stored.
                    name: "fromToken".to_string(),
                    ty: ParamTy::Text {
                        max_len: Some(8 * 1024),
                    },
                    required: true,
                    bounds: None,
                },
                flag_param("activate"),
                reason_param(),
            ],
            &["promoted", "from", "to", "sourceId", "report"],
        ),
        spec(
            SET_ENVIRONMENT_DOOR,
            OpKind::Write,
            vec![
                name_param(),
                optional_text("address", 256),
                optional_text("space", 64),
                optional_text("handle", 63),
                reason_param(),
            ],
            &["name", "door", "own", "chain"],
        ),
        spec(
            SET_PROMOTION_SETTING,
            OpKind::Write,
            vec![
                ParamSpec {
                    name: "setting".to_string(),
                    ty: ParamTy::Text { max_len: Some(64) },
                    required: true,
                    bounds: None,
                },
                ParamSpec {
                    name: "value".to_string(),
                    ty: ParamTy::Text { max_len: Some(8) },
                    required: true,
                    bounds: None,
                },
                reason_param(),
            ],
            &["setting", "value", "changed", "skipStaging"],
        ),
    ]
    .into_iter()
    .chain(crate::evolve::specs())
    .chain(crate::rollouts::specs())
    .chain(crate::orphan_ops::specs())
    .collect()
}

fn optional_text(name: &str, max: u32) -> ParamSpec {
    ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required: false,
        bounds: None,
    }
}

/// The door the params name: `None` when `address` is absent or empty, and
/// a refusal when an address comes without a space.
fn door_of(
    bound: &BoundParams,
) -> Result<Option<crate::environments::Door>, crate::environments::EnvironmentRefusal> {
    let Some(address) = text(bound, "address").filter(|a| !a.trim().is_empty()) else {
        return Ok(None);
    };
    let Some(space) = text(bound, "space").filter(|s| !s.trim().is_empty()) else {
        return Err(crate::environments::EnvironmentRefusal::Door(
            "an address needs the space that deployment answers its deploy ops in: \
             `--space <its ORDO_SPACE>`"
                .to_string(),
        ));
    };
    crate::environments::Door::checked(address, space, text(bound, "handle")).map(Some)
}

fn name_param() -> ParamSpec {
    ParamSpec {
        name: "name".to_string(),
        ty: ParamTy::Text { max_len: Some(128) },
        required: true,
        bounds: None,
    }
}

/// One environment as the ops answer it.
fn environment_json(e: &crate::environments::Environment) -> serde_json::Value {
    json!({
        "name": e.name, "position": e.position, "own": e.own,
        "actor": e.actor, "reason": e.reason, "at": e.at,
        "door": e.door.as_ref().map(|d| json!({
            "address": d.address, "space": d.space, "handle": d.handle,
        })),
    })
}

/// One environment of `std.deployment.environments`' answer, read back: what
/// the CLI uses to work out where `ordo apply` and `ordo promote` go, with
/// the host's own rules ([`crate::environments::check_apply_target`],
/// [`crate::environments::promotion_source`]).
pub fn environment_from_json(v: &serde_json::Value) -> Option<crate::environments::Environment> {
    let door = match (v["door"]["address"].as_str(), v["door"]["space"].as_str()) {
        (Some(address), Some(space)) => Some(crate::environments::Door {
            address: address.to_string(),
            space: space.to_string(),
            handle: v["door"]["handle"].as_str().map(str::to_string),
        }),
        _ => None,
    };
    Some(crate::environments::Environment {
        name: v["name"].as_str()?.to_string(),
        position: u32::try_from(v["position"].as_u64()?).ok()?,
        own: v["own"].as_bool()?,
        actor: v["actor"].as_str().unwrap_or_default().to_string(),
        reason: v["reason"].as_str().unwrap_or_default().to_string(),
        at: v["at"].as_u64().unwrap_or(0),
        door,
    })
}

/// The chain in `std.deployment.environments`' answer.
pub fn chain_from_json(row: &serde_json::Value) -> Vec<crate::environments::Environment> {
    row["chain"]
        .as_array()
        .map(|a| a.iter().filter_map(environment_from_json).collect())
        .unwrap_or_default()
}

/// The three settings, as both ops answer them. Unset reads as `null`.
fn settings_json(s: &crate::versions::VersionSettings) -> serde_json::Value {
    use crate::versions::VersionSetting;
    json!({
        "served": s.value(VersionSetting::Served),
        "minimum": s.value(VersionSetting::Minimum),
        "newTenant": s.value(VersionSetting::NewTenant),
        "minResidency": s.value(VersionSetting::MinResidency),
        "idleEvictAfter": s.value(VersionSetting::IdleEvictAfter),
    })
}

/// **A tenant on the deployment's own database is not pinned apart from
/// the active version across B10** (increment 13): there its field's
/// blind index is under the active version's normalization, and `version`
/// would read the index kept beside it, which only an activation trades
/// in, for every space at once. Refused by name; the node refuses to serve
/// such a pin too (`crate::spaces::Spaces::resolve`), however it was made.
async fn refuse_pin_apart_from_kept(
    ops: &ApplyOps,
    tenant: &str,
    version: &str,
) -> Result<(), ExecError> {
    const WHAT: &str = "the tenant was not moved";
    let v = crate::versions::Versions::on(&ops.pool);
    let Ok(version) = v.resolve(version).await else {
        return Ok(()); // the pin names the refusal
    };
    let space = ordo_crypto::SpaceId(tenant.to_string());
    let Ok(pool) = ops.pools.resolve(&space).await else {
        return Ok(());
    };
    if !crate::dual_writes::same_database(&pool, &ops.pool) {
        return Ok(()); // its own database: the node trades it
    }
    let Some(schema) = v
        .get(&version)
        .await
        .map_err(|e| e.to_exec_error(WHAT))?
        .and_then(|a| crate::Fixture::from_source(&a.source, "").ok())
        .map(|f| f.schema)
    else {
        return Ok(());
    };
    let due = crate::schema_apply::kept_indexes_due(&pool, &schema)
        .await
        .map_err(|e| {
            ExecError::of_fault(
                e.class(),
                ordo_api::ErrorCode::SpaceUnavailable,
                format!("{WHAT}: its database could not be read: {e}"),
            )
        })?;
    match due.first() {
        Some(k) => Err(ExecError::new(
            ordo_api::ErrorCode::ApplyRefused,
            format!(
                "{WHAT}: {}",
                crate::spaces::kept_on_shared(&k.entity, &k.field, &version, tenant)
            ),
        )),
        None => Ok(()),
    }
}

/// The tenants waiting to move off a revoked or too-old version, and how
/// many of them no window covers. Best effort: unreadable counts as none.
async fn waiting_json(pool: &ordo_exec::Pool) -> serde_json::Value {
    let w = crate::revoked_moves::waiting(pool, crate::runtime::now_secs())
        .await
        .unwrap_or_default();
    json!({ "tenants": w.tenants, "noWindow": w.no_window, "line": w.line() })
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
            // A rollback also puts back each tenant soaking on a move from
            // the version, every write kept (increment 11b).
            let activated = apply_op::activate_reverting(
                &ops.pool,
                Some(ops.pools.as_ref()),
                ops.spaces.get().map(|s| s.as_ref()),
                version,
                caller,
            )
            .await;
            let (activation, reverted) = match activated {
                Ok(done) => done,
                Err(refusal) => return Err(ExecError::new(refusal.code(), refusal.to_string())),
            };
            if !reverted.is_empty() {
                ops.record(
                    caller,
                    ACTIVATE,
                    "soaks-reverted",
                    format!(
                        "version={version} tenants={} by={caller}",
                        reverted.join("+")
                    ),
                );
            }
            match activation {
                Activation::Switched { from, to } => {
                    ops.record(
                        caller,
                        ACTIVATE,
                        "activated",
                        format!(
                            "version={to} was={} by={caller}",
                            from.as_deref().unwrap_or("none")
                        ),
                    );
                    json!({ "state": "switched", "version": to, "from": from, "reverted": reverted })
                }
                Activation::AlreadyActive { version } => {
                    json!({
                        "state": "alreadyActive", "version": version, "from": version,
                        "reverted": reverted,
                    })
                }
                Activation::Staged | Activation::RolloutStarted { .. } => {
                    return Err(ExecError::internal("an activation answered staged"))
                }
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
        EVOLVE | PLANS | REPLAY | ROLLBACK_CHECK | REVERT_SOAK | CLOSE_SOAK => {
            // Boxed: a step's future is deep, and a door task's stack is
            // not (a move overflowed it on CI's runners).
            Box::pin(crate::evolve::execute(ops, name, bound, caller)).await?
        }
        name if crate::rollouts::OPS.contains(&name) => {
            crate::rollouts::execute(ops, name, bound, caller).await?
        }
        ORPHANS | PRUNE => crate::orphan_ops::execute(ops, name, bound, caller).await?,
        PIN => {
            let tenant = text(bound, "tenant").unwrap_or_default();
            let version = text(bound, "version").unwrap_or_default();
            let v = crate::versions::Versions::on(&ops.pool);
            refuse_pin_apart_from_kept(ops, tenant, version).await?;
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
            row["rollout"] = crate::rollouts::Rollouts::on(&ops.pool)
                .settings()
                .await
                .map_err(|e| e.to_exec_error("the rollout settings could not be read"))?
                .to_json();
            row["backfill"] = crate::backfill::stored_settings(&ops.pool)
                .await
                .map_err(|e| e.to_exec_error("the backfill settings could not be read"))?
                .to_json();
            row["waiting"] = waiting_json(&ops.pool).await;
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
            // The rollout settings are stored beside the version settings
            // and set through the same op (ruled 2026-11-23, Q15).
            if let Some(setting) = crate::rollouts::setting_name(named)
                .filter(|_| named.trim().starts_with("rollout."))
            {
                let value = text(bound, "value").filter(|v| !v.is_empty());
                let reason = text(bound, "reason").unwrap_or_default();
                let (changed, now) = crate::rollouts::Rollouts::on(&ops.pool)
                    .set_setting(setting, value, caller, reason, crate::runtime::now_secs())
                    .await
                    .map_err(|e| e.to_exec_error("the rollout setting was not changed"))?;
                let value = now.to_json()[crate::rollouts::json_key(setting)].clone();
                if changed {
                    ops.record(
                        caller,
                        SET_VERSION_SETTING,
                        "version-setting",
                        format!("setting={setting} value={value} by={caller}"),
                    );
                }
                let v = crate::versions::Versions::on(&ops.pool);
                let mut row = settings_json(
                    &v.settings()
                        .await
                        .map_err(|e| e.to_exec_error("the version settings could not be read"))?,
                );
                row["rollout"] = now.to_json();
                row["setting"] = json!(setting);
                row["value"] = value;
                row["changed"] = json!(changed);
                row["waiting"] = waiting_json(&ops.pool).await;
                return Ok(ExecResult {
                    position: None,
                    rows: vec![row],
                    next_cursor: None,
                });
            }
            // So are the backfill settings (Q15): what a stored step's
            // backfill runs with.
            if named.trim().starts_with("backfill.") {
                let value = text(bound, "value").filter(|v| !v.is_empty());
                let reason = text(bound, "reason").unwrap_or_default();
                let (changed, now) = crate::backfill::set_setting(
                    &ops.pool,
                    named,
                    value,
                    caller,
                    reason,
                    crate::runtime::now_secs(),
                )
                .await
                .map_err(|e| e.to_exec_error("the backfill setting was not changed"))?;
                let setting = crate::backfill::setting_name(named).unwrap_or_default();
                let value = now.to_json()[match setting {
                    crate::backfill::CHUNK_ROWS_SETTING => "chunkRows",
                    crate::backfill::CHUNK_TIMEOUT_SETTING => "chunkTimeout",
                    crate::backfill::MAX_ATTEMPTS_SETTING => "maxAttempts",
                    crate::backfill::CONCURRENCY_SETTING => "concurrency",
                    crate::backfill::PER_DATABASE_SETTING => "perDatabase",
                    _ => "budget",
                }]
                .clone();
                if changed {
                    ops.record(
                        caller,
                        SET_VERSION_SETTING,
                        "version-setting",
                        format!("setting={setting} value={value} by={caller}"),
                    );
                }
                let v = crate::versions::Versions::on(&ops.pool);
                let mut row = settings_json(
                    &v.settings()
                        .await
                        .map_err(|e| e.to_exec_error("the version settings could not be read"))?,
                );
                row["backfill"] = now.to_json();
                row["setting"] = json!(setting);
                row["value"] = value;
                row["changed"] = json!(changed);
                row["waiting"] = waiting_json(&ops.pool).await;
                return Ok(ExecResult {
                    position: None,
                    rows: vec![row],
                    next_cursor: None,
                });
            }
            let Some(setting) = crate::versions::VersionSetting::parse(named) else {
                return Err(ExecError::new(
                    ordo_api::ErrorCode::InvalidParam,
                    format!(
                        "the version setting was not changed: `{named}` is not one. There are \
                         versions.served, versions.minimum, versions.new_tenant, the \
                         rollout settings {}, and the backfill settings {}",
                        crate::rollouts::SETTINGS.join(", "),
                        crate::backfill::SETTINGS.join(", ")
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
            row["waiting"] = waiting_json(&ops.pool).await;
            row
        }
        ENVIRONMENTS => {
            let read = |e: crate::environments::EnvironmentRefusal| {
                e.to_exec_error("the environments could not be read")
            };
            let envs = crate::environments::Environments::on(&ops.pool);
            let chain = envs.chain().await.map_err(read)?;
            let named = match text(bound, "name").filter(|n| !n.is_empty()) {
                Some(n) => Some(envs.require(n).await.map_err(read)?),
                None => None,
            };
            let settings = envs.promotion_settings().await.map_err(read)?;
            json!({
                "own": chain.iter().find(|e| e.own).map(|e| e.name.clone()),
                "chain": chain.iter().map(environment_json).collect::<Vec<_>>(),
                "environment": named.as_ref().map(environment_json),
                "skipStaging": settings.skip_staging,
            })
        }
        REGISTER_ENVIRONMENT => {
            let name = text(bound, "name").unwrap_or_default();
            let position = bound
                .0
                .get("position")
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok());
            let reason = text(bound, "reason").unwrap_or_default();
            let refused = |e: crate::environments::EnvironmentRefusal| {
                e.to_exec_error("the environment was not registered")
            };
            let door = door_of(bound).map_err(refused)?;
            let chain = crate::environments::Environments::on(&ops.pool)
                .register_with_door(
                    name,
                    position,
                    door,
                    caller,
                    reason,
                    crate::runtime::now_secs(),
                )
                .await
                .map_err(refused)?;
            ops.record(
                caller,
                REGISTER_ENVIRONMENT,
                "environment-registered",
                format!("environment={name} by={caller}"),
            );
            json!({
                "registered": name,
                "own": chain.iter().find(|e| e.own).map(|e| e.name.clone()),
                "chain": chain.iter().map(environment_json).collect::<Vec<_>>(),
            })
        }
        SET_OWN_ENVIRONMENT => {
            let name = text(bound, "name").unwrap_or_default();
            let reason = text(bound, "reason").unwrap_or_default();
            let (chain, changed) = crate::environments::Environments::on(&ops.pool)
                .set_own(name, reason)
                .await
                .map_err(|e| e.to_exec_error("this deployment's environment was not changed"))?;
            if changed {
                ops.record(
                    caller,
                    SET_OWN_ENVIRONMENT,
                    "environment-own",
                    format!("environment={name} by={caller} reason={}", reason.trim()),
                );
            }
            json!({
                "own": name, "changed": changed,
                "chain": chain.iter().map(environment_json).collect::<Vec<_>>(),
            })
        }
        REMOVE_ENVIRONMENT => {
            let name = text(bound, "name").unwrap_or_default();
            let reason = text(bound, "reason").unwrap_or_default();
            let chain = crate::environments::Environments::on(&ops.pool)
                .remove(name, reason)
                .await
                .map_err(|e| e.to_exec_error("the environment was not removed"))?;
            ops.record(
                caller,
                REMOVE_ENVIRONMENT,
                "environment-removed",
                format!("environment={name} by={caller} reason={}", reason.trim()),
            );
            json!({
                "removed": name,
                "own": chain.iter().find(|e| e.own).map(|e| e.name.clone()),
                "chain": chain.iter().map(environment_json).collect::<Vec<_>>(),
            })
        }
        RENAME_ENVIRONMENT => {
            let name = text(bound, "name").unwrap_or_default();
            let to = text(bound, "to").unwrap_or_default();
            let reason = text(bound, "reason").unwrap_or_default();
            let chain = crate::environments::Environments::on(&ops.pool)
                .rename(name, to, reason)
                .await
                .map_err(|e| e.to_exec_error("the environment was not renamed"))?;
            ops.record(
                caller,
                RENAME_ENVIRONMENT,
                "environment-renamed",
                format!(
                    "environment={name} to={to} by={caller} reason={}",
                    reason.trim()
                ),
            );
            json!({
                "renamed": name, "to": to,
                "own": chain.iter().find(|e| e.own).map(|e| e.name.clone()),
                "chain": chain.iter().map(environment_json).collect::<Vec<_>>(),
            })
        }
        VERSION_SOURCE => {
            let asked = text(bound, "version").unwrap_or_default();
            let read = |e: crate::versions::VersionRefusal| {
                e.to_exec_error("the version's source could not be read")
            };
            let v = crate::versions::Versions::on(&ops.pool);
            let version = v.resolve(asked).await.map_err(read)?;
            let stored =
                v.get(&version).await.map_err(read)?.ok_or_else(|| {
                    ExecError::new(ordo_api::ErrorCode::NotFound, "no such version")
                })?;
            let doc: serde_json::Value = serde_json::from_str(&stored.artifact)
                .map_err(|e| ExecError::internal(format!("the stored artifact: {e}")))?;
            let hashes = ordo_plan::artifact::code_hashes(&doc);
            let mut files = apply_op::CodeFiles::new();
            for (path, hash) in &hashes {
                let bytes = crate::functions::load_code(&ops.pool, hash)
                    .await
                    .map_err(|e| ExecError::internal(format!("function code: {e}")))?
                    .ok_or_else(|| {
                        ExecError::internal(format!("the code for `{path}` is not stored"))
                    })?;
                files.insert(
                    path.clone(),
                    apply_op::CodeFile {
                        hash: hash.clone(),
                        text: String::from_utf8_lossy(&bytes).into_owned(),
                    },
                );
            }
            let (_, source_id) = v
                .identity(&version)
                .await
                .map_err(read)?
                .unwrap_or_default();
            let activated = v.activated_at(&version).await.map_err(read)?.is_some();
            let env = crate::environments::Environments::on(&ops.pool)
                .own()
                .await
                .map_err(|e| e.to_exec_error("the environment could not be read"))?;
            json!({
                "env": env.name, "version": version, "source": stored.source,
                "sourceId": source_id, "codeHashes": hashes,
                "code": apply_op::code_text(&files), "activated": activated,
                "readBy": caller,
            })
        }
        PROMOTE => {
            let get = |k: &str| text(bound, k).unwrap_or_default().to_string();
            let activate = bound
                .0
                .get("activate")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let reason = get("reason");
            let version = get("version");
            let credential = crate::promotion::SourceCredential::Person {
                token: get("fromToken"),
            };
            let done = crate::promotion::promote(
                &ops.pool,
                ops.pools.as_ref(),
                crate::promotion::PromoteRequest {
                    version: &version,
                    credential: &credential,
                    reason: &reason,
                    activate,
                },
                caller,
                crate::runtime::now_secs(),
            )
            .await
            .map_err(|e| e.to_exec_error("the version was not promoted"))?;
            ops.record(
                caller,
                PROMOTE,
                "promoted",
                format!(
                    "version={} from={} to={} source={} by={caller} reason={}",
                    done.report.version,
                    done.from,
                    done.to,
                    done.source_id,
                    reason.trim()
                ),
            );
            json!({
                "promoted": done.report.version, "from": done.from, "to": done.to,
                "sourceId": done.source_id, "report": done.report.to_json(),
            })
        }
        SET_ENVIRONMENT_DOOR => {
            let name = text(bound, "name").unwrap_or_default();
            let reason = text(bound, "reason").unwrap_or_default();
            let refused = |e: crate::environments::EnvironmentRefusal| {
                e.to_exec_error("the environment's door was not recorded")
            };
            let door = door_of(bound).map_err(refused)?;
            let chain = crate::environments::Environments::on(&ops.pool)
                .set_door(name, door.clone(), reason)
                .await
                .map_err(refused)?;
            ops.record(
                caller,
                SET_ENVIRONMENT_DOOR,
                "environment-door",
                format!(
                    "environment={name} address={} by={caller} reason={}",
                    door.as_ref().map_or("cleared", |d| d.address.as_str()),
                    reason.trim()
                ),
            );
            let row = chain.iter().find(|e| e.name == name.trim());
            json!({
                "name": name.trim(),
                "door": row.map(environment_json).map(|e| e["door"].clone()),
                "own": chain.iter().find(|e| e.own).map(|e| e.name.clone()),
                "chain": chain.iter().map(environment_json).collect::<Vec<_>>(),
            })
        }
        SET_PROMOTION_SETTING
            if crate::environments::is_prune_setting(
                text(bound, "setting").unwrap_or_default(),
            ) =>
        {
            let value = text(bound, "value").unwrap_or_default();
            let reason = text(bound, "reason").unwrap_or_default();
            let envs = crate::environments::Environments::on(&ops.pool);
            let (on, changed) = envs
                .set_prune_enabled(value, caller, reason, crate::runtime::now_secs())
                .await
                .map_err(|e| e.to_exec_error("the prune setting was not changed"))?;
            let shown = if on { "on" } else { "off" };
            if changed {
                ops.record(
                    caller,
                    SET_PROMOTION_SETTING,
                    "prune-setting",
                    format!(
                        "setting={} value={shown} by={caller} reason={}",
                        crate::environments::PRUNE_ENABLED,
                        reason.trim()
                    ),
                );
            }
            let skip = envs
                .promotion_settings()
                .await
                .map_err(|e| e.to_exec_error("the settings could not be read"))?;
            json!({
                "setting": crate::environments::PRUNE_ENABLED, "value": shown,
                "changed": changed, "skipStaging": skip.skip_staging,
            })
        }
        SET_PROMOTION_SETTING => {
            let setting = text(bound, "setting").unwrap_or_default();
            let value = text(bound, "value").unwrap_or_default();
            let reason = text(bound, "reason").unwrap_or_default();
            let (settings, changed) = crate::environments::Environments::on(&ops.pool)
                .set_promotion_setting(setting, value, caller, reason, crate::runtime::now_secs())
                .await
                .map_err(|e| e.to_exec_error("the promotion setting was not changed"))?;
            let (name, shown) = match crate::environments::rehearsal_setting(setting) {
                Some(name) => (name, value.trim()),
                None => (
                    crate::environments::SKIP_STAGING,
                    if settings.skip_staging { "on" } else { "off" },
                ),
            };
            if changed {
                ops.record(
                    caller,
                    SET_PROMOTION_SETTING,
                    "promotion-setting",
                    format!(
                        "setting={name} value={shown} by={caller} reason={}",
                        reason.trim()
                    ),
                );
            }
            json!({
                "setting": name, "value": shown,
                "changed": changed, "skipStaging": settings.skip_staging,
            })
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
