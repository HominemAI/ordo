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

pub const OPS: [&str; 5] = [VERSIONS, ACTIVATE, PLAN, LABEL, DIFF];

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
    ]
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

/// Run one of the three. `caller` is the principal the door proved.
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
        other => return Err(ExecError::internal(format!("`{other}` is not a deploy op"))),
    };
    Ok(ExecResult {
        position: None,
        rows: vec![row],
        next_cursor: None,
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
