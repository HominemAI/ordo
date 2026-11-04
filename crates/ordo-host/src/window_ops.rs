//! The door's maintenance window ops (owner 2026-11-04, asks 22 and 24),
//! each the body of an `ordo windows` subcommand and what the console's
//! window screens call:
//!
//! - `std.deployment.windows($tenant?)`: every window, skip and time from
//!   12 months back to 12 months ahead, for one tenant or all of them.
//! - `std.deployment.setWindow($id?, $target, $tenants?, $starts?, $ends?,
//!   $cron?, $lengthSecs?, $message)`: create a window, or replace the one
//!   under `$id`. A range takes `$starts` and `$ends`; a cron takes
//!   `$cron` and `$lengthSecs`.
//! - `std.deployment.deleteWindow($id)`.
//! - `std.deployment.skipWindow($window, $id?, $starts, $ends, $reason?)`:
//!   suppress a cron window's times over a range.
//! - `std.deployment.deleteSkip($id)`.
//!
//! The bodies are [`crate::windows::Windows`]', which the CLI calls too.
//! Who may call them is `crate::deploy_ops`' rule: the deployment owner,
//! bound to the deployment's own space, a touch for each change. Each
//! change is written to the security record as a `deploy` event.

use ordo_api::{
    BoundParams, Bounds, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use serde_json::json;

use crate::apply_op::ApplyOps;
use crate::windows::{self, Schedule, Target, WindowSpec, Windows};

/// Every window, skip and time, for one tenant or all.
pub const WINDOWS: &str = "std.deployment.windows";
/// Create or replace a window.
pub const SET_WINDOW: &str = "std.deployment.setWindow";
/// Delete a window and its skips.
pub const DELETE_WINDOW: &str = "std.deployment.deleteWindow";
/// Suppress a cron window's times over a range.
pub const SKIP_WINDOW: &str = "std.deployment.skipWindow";
/// Delete a skip.
pub const DELETE_SKIP: &str = "std.deployment.deleteSkip";

pub const OPS: [&str; 5] = [WINDOWS, SET_WINDOW, DELETE_WINDOW, SKIP_WINDOW, DELETE_SKIP];

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
    let text = |name: &str, max: u32, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    };
    let time = |name: &str, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Datetime,
        required,
        bounds: None,
    };
    vec![
        spec(
            WINDOWS,
            OpKind::Read,
            vec![text("tenant", 128, false)],
            &[
                "from",
                "to",
                "tenant",
                "windows",
                "skips",
                "occurrences",
                "truncated",
            ],
        ),
        spec(
            SET_WINDOW,
            OpKind::Write,
            vec![
                text("id", 64, false),
                // `all`, `only` or `except`.
                text("target", 16, true),
                ParamSpec {
                    // A JSON array of tenant ids, for `only` and `except`.
                    name: "tenants".to_string(),
                    ty: ParamTy::Json,
                    required: false,
                    bounds: None,
                },
                time("starts", false),
                time("ends", false),
                text("cron", 128, false),
                ParamSpec {
                    name: "lengthSecs".to_string(),
                    ty: ParamTy::Int,
                    required: false,
                    bounds: Some(Bounds {
                        min: Some("1".to_string()),
                        max: None,
                    }),
                },
                // At most 2,000 characters, which the `Windows` client
                // checks and names; this bound is only a ceiling.
                text("message", 8192, true),
            ],
            &["id", "target", "tenants", "message", "setBy", "setAt"],
        ),
        spec(
            DELETE_WINDOW,
            OpKind::Write,
            vec![text("id", 64, true)],
            &["id", "deleted"],
        ),
        spec(
            SKIP_WINDOW,
            OpKind::Write,
            vec![
                text("window", 64, true),
                text("id", 64, false),
                time("starts", true),
                time("ends", true),
                text("reason", 2048, false),
            ],
            &["id", "window", "starts", "ends", "reason", "setBy", "setAt"],
        ),
        spec(
            DELETE_SKIP,
            OpKind::Write,
            vec![text("id", 64, true)],
            &["id", "deleted"],
        ),
    ]
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
    let w = Windows::on(&ops.pool);
    let now = i64::try_from(crate::runtime::now_secs()).unwrap_or(i64::MAX);
    let time = |k: &str| -> Result<Option<i64>, ExecError> {
        text(bound, k)
            .map(windows::parse_utc)
            .transpose()
            .map_err(|e| e.to_exec_error(&format!("`{k}`")))
    };
    let row = match name {
        WINDOWS => w
            .list(text(bound, "tenant"), now)
            .await
            .map_err(|e| e.to_exec_error("the windows could not be listed"))?
            .to_json(),
        SET_WINDOW => {
            let refused = |e: windows::WindowRefusal| e.to_exec_error("the window was not set");
            let tenants: Vec<String> = match bound.0.get("tenants") {
                None | Some(serde_json::Value::Null) => vec![],
                Some(serde_json::Value::Array(a)) => a
                    .iter()
                    .map(|t| t.as_str().map(str::to_string))
                    .collect::<Option<_>>()
                    .ok_or_else(|| {
                        refused(windows::WindowRefusal::Input(
                            "`tenants` is a JSON array of tenant ids".into(),
                        ))
                    })?,
                Some(_) => {
                    return Err(refused(windows::WindowRefusal::Input(
                        "`tenants` is a JSON array of tenant ids".into(),
                    )))
                }
            };
            let target = Target::parse(text(bound, "target").unwrap_or_default(), &tenants)
                .map_err(refused)?;
            let length = bound.0.get("lengthSecs").and_then(|v| v.as_i64());
            let schedule = match (time("starts")?, time("ends")?, text(bound, "cron"), length) {
                (Some(start), Some(end), None, None) => Schedule::Range { start, end },
                (None, None, Some(cron), Some(length)) => Schedule::Cron {
                    cron: cron.to_string(),
                    length,
                },
                _ => {
                    return Err(refused(windows::WindowRefusal::Input(
                        "a window is a range (`starts` and `ends`) or a cron (`cron` and \
                         `lengthSecs`), one or the other"
                            .into(),
                    )))
                }
            };
            let set = w
                .set(
                    &WindowSpec {
                        id: text(bound, "id")
                            .filter(|i| !i.is_empty())
                            .map(str::to_string),
                        target,
                        schedule,
                        message: bound
                            .0
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    },
                    caller,
                    now,
                )
                .await
                .map_err(refused)?;
            ops.record(
                caller,
                SET_WINDOW,
                "window-set",
                format!("window={} target={} by={caller}", set.id, set.target.kind()),
            );
            set.to_json()
        }
        DELETE_WINDOW => {
            let id = text(bound, "id").unwrap_or_default();
            w.delete(id)
                .await
                .map_err(|e| e.to_exec_error("the window was not deleted"))?;
            ops.record(
                caller,
                DELETE_WINDOW,
                "window-deleted",
                format!("window={id} by={caller}"),
            );
            json!({ "id": id, "deleted": true })
        }
        SKIP_WINDOW => {
            let refused = |e: windows::WindowRefusal| e.to_exec_error("the skip was not set");
            let (Some(start), Some(end)) = (time("starts")?, time("ends")?) else {
                return Err(refused(windows::WindowRefusal::Input(
                    "a skip takes `starts` and `ends`".into(),
                )));
            };
            let skip = w
                .skip(
                    text(bound, "window").unwrap_or_default(),
                    text(bound, "id").filter(|i| !i.is_empty()),
                    start,
                    end,
                    text(bound, "reason"),
                    caller,
                    now,
                )
                .await
                .map_err(refused)?;
            ops.record(
                caller,
                SKIP_WINDOW,
                "window-skipped",
                format!("skip={} window={} by={caller}", skip.id, skip.window),
            );
            skip.to_json()
        }
        DELETE_SKIP => {
            let id = text(bound, "id").unwrap_or_default();
            w.delete_skip(id)
                .await
                .map_err(|e| e.to_exec_error("the skip was not deleted"))?;
            ops.record(
                caller,
                DELETE_SKIP,
                "skip-deleted",
                format!("skip={id} by={caller}"),
            );
            json!({ "id": id, "deleted": true })
        }
        other => return Err(ExecError::internal(format!("`{other}` is not a window op"))),
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

    #[test]
    fn the_list_is_a_read_and_the_rest_are_changes() {
        let specs = specs();
        let kind = |n: &str| specs.iter().find(|s| s.name == n).map(|s| s.kind);
        assert_eq!(kind(WINDOWS), Some(OpKind::Read));
        for op in [SET_WINDOW, DELETE_WINDOW, SKIP_WINDOW, DELETE_SKIP] {
            assert_eq!(kind(op), Some(OpKind::Write), "{op}");
        }
        assert_eq!(specs.len(), OPS.len());
    }
}
