//! A stand-in for a tenant's links and shares, its own security record and
//! the deployment's (G8), for the console's tests and its end-to-end stack
//! (`examples/e2e_door.rs` includes this file).
//!
//! The door's check stays the real one (`ordo_host::IdentityAccess`, with
//! its home space and its rule that a console session leaves the home
//! space only for the listed ops with a fresh touch): this only answers
//! once the door allowed the call, over memory, in the shapes `ordo-host`
//! answers (`std.space.links`, `shares`, `revokeLink`, `revokeShare`,
//! `GET /security/events`, `std.deployment.securityEvents`).
//! `ordo-host/tests/deployment_security.rs`, `share_links.rs` and
//! `sharing.rs` cover the real ones against Postgres.

#![allow(dead_code)]

use std::sync::Mutex;

use ordo_identity::{AccessEngine, AccessProfile, PrincipalId, Role, SpaceId};
use serde_json::{json, Value};

pub const LINKS: &str = "std.space.links";
pub const SHARES: &str = "std.space.shares";
pub const REVOKE_LINK: &str = "std.space.revokeLink";
pub const REVOKE_SHARE: &str = "std.space.revokeShare";
pub const EVENTS: &str = "std.deployment.securityEvents";
/// The op `GET /security/events` is checked against, in the space named.
pub const SECEVENTS_OP: &str = "platform.security.events";

/// The ops this stand-in answers at `/op/{name}`.
pub const OPS: [&str; 5] = [LINKS, SHARES, REVOKE_LINK, REVOKE_SHARE, EVENTS];
/// The tenant the deployment owner owns (`tenants_door::MINE`).
pub const MINE: &str = "spc_mine";

/// The seeded links, by id.
pub const LIVE: &str = "lnk_live";
pub const USED_UP: &str = "lnk_used";
pub const GONE: &str = "lnk_gone";
/// The seeded share's seat.
pub const SEAT: &str = "mem_guest";

pub struct Shares {
    links: Mutex<Vec<Value>>,
    shares: Mutex<Vec<Value>>,
    deployment: Mutex<Vec<Value>>,
    tenant: Mutex<Vec<Value>>,
    at: u64,
}

fn link(id: &str, label: &str, state: &str, opens: u64, devices: u64, at: u64) -> Value {
    json!({
        "id": id, "label": label, "ops": ["std.notes.get"], "rows": null,
        "opens": opens, "createdAt": at - 3600, "expiresAt": at + 86_400,
        "state": state, "issuedBy": "mem_owner", "hasPassword": id == LIVE,
        "lockedUntil": if id == LIVE { json!(at + 600) } else { Value::Null },
        "opensUsed": devices,
        "maxOpens": if id == USED_UP { json!(devices) } else { Value::Null },
    })
}

fn event(index: usize, kind: &str, act: Option<&str>, tenant: Option<&str>, at: u64) -> Value {
    json!({
        "index": index, "kind": kind, "act": act,
        "detail": match act { Some(a) => format!("{a} by=dep_owner"), None => String::new() },
        "decisions": if kind == "denied" { json!(["NoMembership"]) } else { Value::Null },
        "subject": "user:dep_owner", "subjectKind": "member",
        "op": "std.deployment.apply", "tenant": tenant,
        "firstAt": at, "lastAt": at, "count": 1,
    })
}

impl Shares {
    pub fn new(at: u64) -> Shares {
        Shares {
            links: Mutex::new(vec![
                link(LIVE, "Board for Ana", "live", 5, 3, at),
                link(USED_UP, "One look", "used-up", 2, 2, at),
                link(GONE, "Old", "revoked", 0, 0, at),
            ]),
            shares: Mutex::new(vec![json!({
                "seat": SEAT, "displayName": "Gil", "guest": true,
                "ops": ["std.notes.get"], "grantedAt": at - 100, "expiresAt": at + 3600,
            })]),
            deployment: Mutex::new(vec![
                event(0, "deploy", Some("applied"), None, at - 50),
                event(1, "denied", None, Some(MINE), at - 40),
                event(2, "enrolment", Some("tenant-paused"), Some(MINE), at - 30),
                event(3, "console-key", Some("added"), None, at - 20),
                event(
                    4,
                    "enrolment",
                    Some("quota-set"),
                    Some("spc_theirs"),
                    at - 10,
                ),
            ]),
            tenant: Mutex::new(vec![json!({
                "index": 0, "subject": "user:mem_x", "subjectKind": "member",
                "op": LINKS, "kind": "denied", "firstAt": at - 5, "lastAt": at - 5,
                "count": 2, "correlations": [],
            })]),
            at,
        }
    }

    /// The owner reads the deployment's record in the home space.
    pub fn home_profile(profile: AccessProfile) -> AccessProfile {
        profile.allow(Role::Owner, EVENTS)
    }

    /// The owner's tenant reaches its links, shares and record. Called
    /// after `Tenants::grant`, which sets that tenant's profile.
    pub fn grant(engine: &mut AccessEngine, owner: Option<&PrincipalId>) {
        let s = SpaceId(MINE.into());
        let mut p = engine
            .profile(&s)
            .cloned()
            .unwrap_or_else(AccessProfile::new);
        for op in [LINKS, SHARES, REVOKE_LINK, REVOKE_SHARE, SECEVENTS_OP] {
            p = p.allow(Role::Owner, op);
        }
        engine.set_profile(&s, p);
        if let Some(o) = owner {
            engine.set_membership(&s, o, Role::Owner);
        }
        engine.require_step_up(REVOKE_LINK);
        engine.require_step_up(REVOKE_SHARE);
    }

    pub fn link_state(&self, id: &str) -> Option<String> {
        self.links
            .lock()
            .unwrap()
            .iter()
            .find(|l| l["id"] == id)
            .and_then(|l| l["state"].as_str().map(str::to_string))
    }

    pub fn share_count(&self) -> usize {
        self.shares.lock().unwrap().len()
    }

    /// Answer one op the door allowed. `Err` is the door's code and message.
    pub fn execute(
        &self,
        name: &str,
        params: &serde_json::Map<String, Value>,
    ) -> Result<Vec<Value>, (String, String)> {
        let text = |k: &str| params.get(k).and_then(Value::as_str).unwrap_or("");
        let num = |k: &str| {
            params.get(k).and_then(|v| match v {
                Value::Number(n) => n.as_u64(),
                Value::String(s) => s.parse().ok(),
                _ => None,
            })
        };
        match name {
            LINKS => Ok(self.links.lock().unwrap().clone()),
            SHARES => Ok(self.shares.lock().unwrap().clone()),
            REVOKE_LINK => {
                let mut links = self.links.lock().unwrap();
                let Some(l) = links.iter_mut().find(|l| l["id"] == text("link")) else {
                    return Err(("not_found".into(), "this tenant has no such link".into()));
                };
                l["state"] = json!("revoked");
                l["lockedUntil"] = Value::Null;
                let mut tenant = self.tenant.lock().unwrap();
                let index = tenant.len();
                tenant.push(json!({
                    "index": index, "subject": "user:dep_owner", "subjectKind": "member",
                    "op": "enrol.link-revoked", "kind": "enrolment",
                    "firstAt": self.at, "lastAt": self.at, "count": 1, "correlations": [],
                }));
                Ok(vec![json!({"id": text("link"), "outcome": "link revoked"})])
            }
            REVOKE_SHARE => {
                let mut shares = self.shares.lock().unwrap();
                let before = shares.len();
                shares.retain(|s| s["seat"] != text("seat"));
                if shares.len() == before {
                    return Err(("not_found".into(), "this tenant has no such seat".into()));
                }
                Ok(vec![
                    json!({"seat": text("seat"), "outcome": "share revoked"}),
                ])
            }
            EVENTS => {
                let all = self.deployment.lock().unwrap();
                let limit = num("limit").unwrap_or(50) as usize;
                let before = num("before").map(|b| b as usize);
                let rows = all
                    .iter()
                    .rev()
                    .filter(|e| before.is_none_or(|b| e["index"].as_u64().unwrap() < b as u64))
                    .filter(|e| params.get("kind").is_none_or(|k| e["kind"] == *k))
                    .filter(|e| params.get("tenant").is_none_or(|t| e["tenant"] == *t))
                    .take(limit)
                    .cloned()
                    .collect();
                Ok(rows)
            }
            other => Err(("not_found".into(), format!("no op `{other}`"))),
        }
    }

    /// `GET /security/events?space&from&limit`: one tenant's record.
    pub fn tenant_events(&self, space: &str, from: usize, limit: usize) -> Value {
        let all = self.tenant.lock().unwrap();
        let events: Vec<Value> = all.iter().skip(from).take(limit).cloned().collect();
        json!({
            "space": space,
            "verifiableFrom": 0,
            "from": from,
            "next": from + events.len(),
            "events": events,
        })
    }
}
