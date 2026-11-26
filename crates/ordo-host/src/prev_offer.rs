//! **The offer to remove landed `prev` lines** (ruled 2026-10-04,
//! `syntax-additions` ask 1; `plan-versions.md` §2.3 and increment 15).
//!
//! A clean `ordo apply` ends by naming each `prev` mark whose rename has
//! landed in every environment, so the developer can remove it. Here a mark
//! has landed when all of these hold:
//!
//! - every version a tenant here is pinned to has the new name, so no
//!   tenant still needs the stored rename;
//! - this deployment is the last environment of its chain. A promotion
//!   reaches it only through every step before it (§3.1), so a rename that
//!   has landed here has landed in every environment.
//!
//! The answer is advice. It changes nothing; the CLI offers the edit.

use ordo_exec::Pool;
use ordo_schema::prev::PrevMark;

use crate::schema_apply::Program;
use crate::versions::Versions;

/// The marks of `program` that have landed in every environment. Any read
/// that fails answers none: the offer is advice, and a wrong "remove it"
/// would cost a lagging tenant its rename.
pub async fn landed(pool: &Pool, program: &Program) -> Vec<PrevMark> {
    let stale = program.schema.prev.clone();
    if stale.is_empty() {
        return Vec::new();
    }
    match crate::environments::Environments::on(pool).chain().await {
        Ok(chain) => {
            let last = chain.iter().map(|e| e.position).max();
            let own = chain.iter().find(|e| e.own).map(|e| e.position);
            if own.is_none() || own != last {
                return Vec::new();
            }
        }
        Err(_) => return Vec::new(),
    }
    let v = Versions::on(pool);
    let Ok(pinned) = v.tenants_per_version().await else {
        return Vec::new();
    };
    let mut schemas = Vec::new();
    for version in pinned.keys() {
        let Ok(Some(a)) = v.get(version).await else {
            return Vec::new();
        };
        let Ok(p) = Program::from_artifact(&a) else {
            return Vec::new();
        };
        schemas.push(p.schema);
    }
    stale
        .into_iter()
        .filter(|m| {
            schemas.iter().all(|s| match &m.field {
                None => s.native(&m.entity).is_some(),
                Some(f) => s.native(&m.entity).is_some_and(|e| e.field(f).is_some()),
            })
        })
        .collect()
}
