//! A stand-in for the deployment's tenant ops, for the console's tests and
//! its end-to-end stack (`examples/e2e_door.rs` includes this file).
//!
//! The door's check stays the real one (`ordo_host::IdentityAccess`, with
//! its home space): this only answers the ops once the door allowed them,
//! over a list in memory, in the shapes `ordo-host` answers
//! (`std.deployment.tenants`, `std.space.create`, `reap`, the pause pair,
//! export, delete and the bucket pair, and owner succession: `openCases`,
//! `succeedOwner` and `resolveObjection`). `ordo-host/tests/deployment_tenants.rs`
//! and `ordo-host/tests/recovery_ops.rs` cover the real ones against Postgres.

#![allow(dead_code)]

use std::sync::Mutex;

use ordo_identity::{AccessEngine, AccessProfile, PrincipalId, Role, SpaceId};
use serde_json::{json, Value};

pub const LIST: &str = "std.deployment.tenants";
pub const CREATE: &str = "std.space.create";
pub const REAP: &str = "std.space.reap";
pub const PAUSE: &str = "std.deployment.pauseTenant";
pub const RESUME: &str = "std.deployment.resumeTenant";
pub const EXPORT: &str = "std.space.export";
pub const DELETE: &str = "std.space.delete";
pub const ATTACH: &str = "std.space.attachBucket";
pub const DETACH: &str = "std.space.detachBucket";
pub const OPEN_CASES: &str = "std.identity.openCases";
pub const SUCCEED: &str = "std.space.succeedOwner";
pub const RESOLVE: &str = "std.identity.resolveObjection";

/// The ops the deployment owner calls in the home space.
pub const HOME_OPS: [&str; 8] = [
    LIST, CREATE, REAP, PAUSE, RESUME, OPEN_CASES, SUCCEED, RESOLVE,
];
/// The reads among them, which ask for no touch.
const READS: [&str; 2] = [LIST, OPEN_CASES];

/// A seeded succession of [`THEIRS`] that has matured.
pub const CASE_MATURED: &str = "rec_matured";
/// A seeded succession of [`HELD`] that a manager objected to.
pub const CASE_PAUSED: &str = "rec_paused";
/// The evidence classes the door takes.
const EVIDENCE: [&str; 4] = [
    "vouched-by-members",
    "verified-out-of-band",
    "contract-holder",
    "host-break-glass",
];
/// The ops the owner of a tenant calls in it.
pub const TENANT_OPS: [&str; 4] = [EXPORT, DELETE, ATTACH, DETACH];

/// The seeded tenant the deployment owner owns.
pub const MINE: &str = "spc_mine";
/// A seeded tenant the deployment owner holds no role in.
pub const THEIRS: &str = "spc_theirs";
/// A seeded tenant stuck in provisioning.
pub const STUCK: &str = "spc_stuck";
/// A seeded tenant the deployment holds closed (`/health/ready`).
pub const HELD: &str = "spc_held";

pub struct Tenants {
    rows: Mutex<Vec<Value>>,
    next: Mutex<u64>,
    /// The open cases, in `std.identity.openCases`' shape.
    cases: Mutex<Vec<Value>>,
}

fn case(id: &str, tenant: &str, principal: &str, at: u64, mature: bool, paused: bool) -> Value {
    json!({
        "case": id, "path": "succession", "tenant": tenant, "principal": principal,
        "evidence": "contract-holder", "reference": "MSA-1",
        "openedAt": at - 90_000, "maturesAt": if mature { at - 3_600 } else { at + 86_400 },
        "state": "pending", "mature": mature && !paused, "paused": paused,
        "approvals": 1, "approvalsRequired": 1, "departed": ["prn_gone"],
        "objections": if paused {
            json!([{"objector": "prn_mia", "tenant": tenant, "at": at - 600,
                    "resolvedAt": null, "resolution": null, "reason": null}])
        } else { json!([]) },
    })
}

fn tenant(id: &str, name: &str, status: &str, at: u64, role: Option<&str>) -> Value {
    json!({
        "id": id, "name": name, "status": status,
        "phase": if status == "provisioning" { "custody" } else { "founded" },
        "createdAt": at, "stateAt": at, "own": false, "note": null,
        "pausedAt": null, "pausedBy": null, "pausedNote": null,
        "target": "default", "custody": "platform",
        "members": if status == "active" { json!(3) } else { Value::Null },
        "usage": if status == "active" {
            json!({"periodStart": 0, "periodEnd": 3600, "writes": 42, "rowsWritten": 50,
                   "bytesSealed": 1000, "bytesChain": 2000, "bytesBlob": 0,
                   "bytesRowsEstimated": 5000})
        } else { Value::Null },
        "quarantined": null,
        "yourRole": role,
        "bucket": null,
    })
}

impl Default for Tenants {
    fn default() -> Self {
        Tenants::new(1_800_000_000)
    }
}

impl Tenants {
    pub fn new(at: u64) -> Tenants {
        Tenants {
            rows: Mutex::new(vec![
                tenant(MINE, "Acme", "active", at - 300, Some("owner")),
                tenant(THEIRS, "Globex", "active", at - 200, None),
                tenant(STUCK, "Half built", "provisioning", at - 100, None),
                tenant(HELD, "Held Ltd", "active", at - 50, None),
            ]),
            next: Mutex::new(1),
            cases: Mutex::new(vec![
                case(CASE_MATURED, THEIRS, "prn_bob", at, true, false),
                case(CASE_PAUSED, HELD, "prn_ann", at, true, true),
            ]),
        }
    }

    /// What `/health/ready` names as held closed.
    pub fn quarantined(&self) -> Value {
        json!([{"space": HELD, "reason": "the chain moved backwards"}])
    }

    pub fn status_of(&self, id: &str) -> Option<String> {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .and_then(|r| r["status"].as_str().map(str::to_string))
    }

    /// The door's rules for these ops: the owner's in the home space
    /// ([`Tenants::home_profile`]), and in each tenant the owner holds the
    /// owner role in ([`Tenants::grant`]). Every write asks for a touch, as
    /// the deployment's default does.
    pub fn home_profile(profile: AccessProfile) -> AccessProfile {
        HOME_OPS
            .iter()
            .fold(profile, |p, op| p.allow(Role::Owner, op))
    }

    /// Step-up on every write, and each tenant the owner owns.
    pub fn grant(&self, engine: &mut AccessEngine, owner: Option<&PrincipalId>) {
        for op in HOME_OPS.iter().chain(TENANT_OPS.iter()) {
            if !READS.contains(op) {
                engine.require_step_up(op);
            }
        }
        let mine: Vec<String> = self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["yourRole"] == "owner")
            .filter_map(|r| r["id"].as_str().map(str::to_string))
            .collect();
        for id in mine {
            let s = SpaceId(id);
            let mut p = AccessProfile::new();
            for op in TENANT_OPS {
                p = p.allow(Role::Owner, op);
            }
            engine.set_profile(&s, p);
            if let Some(o) = owner {
                engine.set_membership(&s, o, Role::Owner);
            }
        }
    }

    /// Answer one op the door allowed. `Err` is the door's code and message.
    pub fn execute(
        &self,
        name: &str,
        space: &str,
        params: &serde_json::Map<String, Value>,
        caller: &str,
    ) -> Result<Vec<Value>, (String, String)> {
        let text = |k: &str| {
            params
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let mut rows = self.rows.lock().unwrap();
        let refused = |m: &str| Err(("tenant_refused".to_string(), m.to_string()));
        let find = |rows: &mut Vec<Value>, id: &str| rows.iter().position(|r| r["id"] == id);
        match name {
            LIST => Ok(rows
                .iter()
                .filter(|r| r["status"] != "reaped")
                .cloned()
                .collect()),
            CREATE => {
                let mut n = self.next.lock().unwrap();
                let id = format!("spc_new{n:02}");
                *n += 1;
                let owner = text("owner");
                let mine = owner == caller;
                rows.insert(
                    0,
                    tenant(
                        &id,
                        &text("name"),
                        "active",
                        1_800_000_000,
                        mine.then_some("owner"),
                    ),
                );
                Ok(vec![
                    json!({"space": id, "name": text("name"), "owner": owner, "reachable": true}),
                ])
            }
            REAP => {
                let id = text("tenant");
                let Some(i) = find(&mut rows, &id) else {
                    return Err(("not_found".into(), "no such tenant".into()));
                };
                if rows[i]["status"] == "active" || rows[i]["status"] == "paused" {
                    return refused("a serving tenant is never reaped; delete it instead");
                }
                rows[i]["status"] = json!("reaped");
                Ok(vec![
                    json!({"space": id, "outcome": "reaped", "released": []}),
                ])
            }
            PAUSE | RESUME => {
                let id = text("tenant");
                let Some(i) = find(&mut rows, &id) else {
                    return Err(("not_found".into(), "no such tenant".into()));
                };
                if name == PAUSE {
                    let note = params
                        .get("note")
                        .and_then(Value::as_str)
                        .unwrap_or("other")
                        .to_string();
                    if !["owner", "billing", "abuse", "legal", "maintenance", "other"]
                        .contains(&note.as_str())
                    {
                        return refused("pause_note: a pause is recorded with one word");
                    }
                    rows[i]["status"] = json!("paused");
                    rows[i]["pausedAt"] = json!(1_800_000_000u64);
                    rows[i]["pausedBy"] = json!("deployment");
                    rows[i]["pausedNote"] = json!(note);
                } else {
                    rows[i]["status"] = json!("active");
                    rows[i]["pausedAt"] = Value::Null;
                    rows[i]["pausedBy"] = Value::Null;
                    rows[i]["pausedNote"] = Value::Null;
                }
                Ok(vec![
                    json!({"tenant": id, "state": rows[i]["status"], "outcome": name}),
                ])
            }
            EXPORT => Ok(vec![json!({
                "space": space, "version": 1, "sealed": "plaintext",
                "bundle": format!("/var/lib/ordo/exports/{space}-1"),
                "members": 3, "entities": 4, "rows": 50, "logGroups": 9,
            })]),
            DELETE => {
                if text("confirm") != space {
                    return refused("confirm: type the space's own id");
                }
                if let Some(i) = find(&mut rows, space) {
                    rows.remove(i);
                }
                Ok(vec![json!({"space": space, "deleted": true})])
            }
            ATTACH => {
                let Some(i) = find(&mut rows, space) else {
                    return Err(("not_found".into(), "no such tenant".into()));
                };
                rows[i]["bucket"] = json!(text("bucket"));
                Ok(vec![json!({
                    "space": space, "bucket": text("bucket"), "endpoint": text("endpoint"),
                    "outcome": "attached",
                })])
            }
            DETACH => {
                let Some(i) = find(&mut rows, space) else {
                    return Err(("not_found".into(), "no such tenant".into()));
                };
                rows[i]["bucket"] = Value::Null;
                Ok(vec![json!({"space": space, "outcome": "detached"})])
            }
            OPEN_CASES => Ok(self.cases.lock().unwrap().clone()),
            SUCCEED => {
                drop(rows);
                self.succeed(params)
            }
            RESOLVE => {
                drop(rows);
                self.resolve(params)
            }
            other => Err(("not_found".into(), format!("no op {other}"))),
        }
    }

    /// `std.space.succeedOwner`: without `case` it opens one, with it it
    /// carries a matured, unpaused one out.
    fn succeed(
        &self,
        params: &serde_json::Map<String, Value>,
    ) -> Result<Vec<Value>, (String, String)> {
        let text = |k: &str| {
            params
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let refused = |m: &str| Err(("tenant_refused".to_string(), m.to_string()));
        let mut cases = self.cases.lock().unwrap();
        let tenant = text("tenant");
        if params.get("case").is_none() {
            let departed: Vec<Value> = params
                .get("departed")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if departed.is_empty() || departed.iter().any(|d| d == &json!(text("principal"))) {
                return refused(
                    "name who has left: `departed` is a list of 1 to 10 principal ids, each an \
                     owner of the tenant, never the successor",
                );
            }
            if !EVIDENCE.contains(&text("evidence").as_str()) {
                return refused(&format!("evidence is one of: {}", EVIDENCE.join(", ")));
            }
            let mut n = self.next.lock().unwrap();
            let id = format!("rec_new{n:02}");
            *n += 1;
            let mut c = case(
                &id,
                &tenant,
                &text("principal"),
                1_800_000_000,
                false,
                false,
            );
            c["reference"] = json!(text("reference"));
            c["departed"] = json!(departed);
            let matures = c["maturesAt"].clone();
            cases.push(c);
            return Ok(vec![json!({
                "case": id, "tenant": tenant, "principal": text("principal"),
                "maturesAt": matures, "departed": departed, "outcome": "opened",
            })]);
        }
        let id = text("case");
        let Some(i) = cases.iter().position(|c| c["case"] == id.as_str()) else {
            return refused("that succession has not matured, does not exist, or is not one");
        };
        if cases[i]["paused"] == true {
            return refused(
                "that case is paused by a manager's objection. The deployment owner resolves it \
                 with std.identity.resolveObjection (resume or cancel, with a reason)",
            );
        }
        if cases[i]["mature"] != true {
            return refused("that succession has not matured, does not exist, or is not one");
        }
        if cases[i]["tenant"] != tenant.as_str() {
            return refused("that case is about another tenant");
        }
        let c = cases.remove(i);
        let ends = params
            .get("endGrants")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        Ok(vec![json!({
            "case": id, "tenant": tenant, "principal": c["principal"], "member": "mem_bob",
            "from": "editor", "role": "owner", "departed": c["departed"], "endsGrants": ends,
            "grantsEnded": if ends { json!(["cg_1@agent_x"]) } else { json!([]) },
            "delegationsEnded": if ends { json!(["dlg_1@agent_x"]) } else { json!([]) },
            "notEnded": [], "outcome": "succeeded", "door": "rebuilt here",
        })])
    }

    /// `std.identity.resolveObjection`: resume or cancel a paused case.
    fn resolve(
        &self,
        params: &serde_json::Map<String, Value>,
    ) -> Result<Vec<Value>, (String, String)> {
        let text = |k: &str| {
            params
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let refused = |m: &str| Err(("tenant_refused".to_string(), m.to_string()));
        let (id, decision, reason) = (text("case"), text("decision"), text("reason"));
        let reasons: &[&str] = match decision.as_str() {
            "resume" => &[
                "evidence-rechecked",
                "objector-withdrew",
                "objector-answered",
            ],
            "cancel" => &["objection-upheld", "opened-in-error", "not-verified"],
            _ => return refused("a decision is `resume` or `cancel`"),
        };
        if !reasons.contains(&reason.as_str()) {
            return refused(&format!(
                "a reason to {decision} is one of: {}",
                reasons.join(", ")
            ));
        }
        let mut cases = self.cases.lock().unwrap();
        let Some(i) = cases
            .iter()
            .position(|c| c["case"] == id.as_str() && c["paused"] == true)
        else {
            return refused("that case does not exist, is not open, or has no objection waiting");
        };
        let state = if decision == "cancel" {
            cases.remove(i);
            "cancelled"
        } else {
            let c = &mut cases[i];
            c["paused"] = json!(false);
            c["mature"] = json!(true);
            if let Some(o) = c["objections"].as_array_mut() {
                for o in o {
                    o["resolvedAt"] = json!(1_800_000_000u64);
                    o["resolution"] = json!("resume");
                    o["reason"] = json!(reason);
                }
            }
            "pending"
        };
        Ok(vec![json!({
            "case": id, "decision": decision, "reason": reason, "objections": 1,
            "state": state, "outcome": "resolved",
        })])
    }
}
