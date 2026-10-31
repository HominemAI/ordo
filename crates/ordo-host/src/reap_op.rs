//! `std.space.reap` — release what a failed provision left behind
//! (`bootstrap.md` §4.5, checklist 11-15).
//!
//! # Why this is a different op from `std.space.delete`
//!
//! §4.5 is explicit, and the distinction is the whole design: *"Reaping an
//! `active` space is not this op; that is `space.delete`, which is
//! owner-only, step-up gated, and the crypto-erasure path with a
//! confirmation window. **The two are deliberately different ops with
//! different callers**, because 'clean up a failed provision' and 'destroy
//! a customer's world' must never be one button."*
//!
//! So this one refuses any space that is `active`, and it is the
//! deployment operator's rather than a space owner's — a failed provision
//! has no owner to ask, which is the situation that makes it necessary.
//!
//! # What it undoes, and in what order
//!
//! §4.3's compensation table, read backwards from the phase the registry
//! row records. Creation goes registry, target, custody, database, attach,
//! found; reaping goes attach, database, custody, registry.
//!
//! **Each step is idempotent and none of them is fatal to the next.** A
//! reap exists because something already went wrong, so a compensation
//! that refused to continue past its first problem would be a cleanup that
//! needs a cleanup. Every step records what it did, the outcomes land on
//! the registry row, and a step that could not finish leaves the space in
//! `failed` with the reason rather than in `reaped` with a lie.
//!
//! # What it does not do
//!
//! **It does not touch a BYO database's contents.** §4.3: *"a BYO schema
//! that cannot be cleaned is left alone and reported — the platform never
//! issues an unattended `DROP SCHEMA` against a customer's database."* A
//! tenant on `OwnPerimeter` custody gets its attachment and its keys
//! released; its database is reported and left standing, because the
//! platform did not create it and does not own it.

use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_crypto::{KeyProvider, SpaceId};
use ordo_exec::spacedb::{self, DbCustody};
use ordo_exec::{Pool, PoolConfig};
use serde_json::json;

use crate::tenants::{self, State};

/// `std.space.reap` — deployment-scoped, operator-only, step-up gated.
pub const REAP: &str = "std.space.reap";

pub struct ReapOps {
    pub pool: Pool,
    pub keys: Arc<dyn KeyProvider>,
    /// The deployment's own space. Reaping is an act **on the deployment**,
    /// so the op answers only when bound to it, for `TenantOps::create`'s
    /// reason: owning a space you were given is not owning the deployment
    /// that serves it.
    pub home: String,
    /// Credentials and pool shape for dialling a target to drop its
    /// database. The same pair `provision::Target` carries.
    pub credentials: Arc<dyn ordo_exec::route::DbCredentials>,
    pub template: PoolConfig,
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl ReapOps {
    /// `std.space.reap($space)`.
    pub async fn reap(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if space != self.home {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "reaping a tenant is the deployment's act rather than a space's: this op \
                 answers only when it is bound to the deployment's own space",
            ));
        }
        let target = bound
            .0
            .get("tenant")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ExecError::new(
                    ErrorCode::TenantRefused,
                    "`tenant` names the space to reap, and reaping without naming one is not \
                     a thing this op does",
                )
            })?;

        let tenant = tenants::lookup(&self.pool, target)
            .await
            .map_err(|e| {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
            })?
            .ok_or_else(|| {
                ExecError::new(
                    ErrorCode::NotFound,
                    "this deployment has no record of that tenant, so there is nothing of \
                     its making to release",
                )
            })?;

        // **An active tenant is somebody's world.** §4.5 puts the whole
        // weight of the design on this line: cleaning up a failed
        // provision and destroying a customer's data must never be one
        // button, and the button is chosen here.
        // A paused tenant is held still on purpose and is somebody's world
        // as much as an active one (27ar).
        if matches!(tenant.state, State::Active | State::Paused) {
            return Err(ExecError::new(
                ErrorCode::TenantRefused,
                "that tenant is active or paused, and reaping is for a provision that failed. \
                 Destroying an active space is `std.space.delete`, which is the owner's \
                 and goes through crypto-erasure with a confirmation window",
            ));
        }
        if tenant.state == State::Reaped {
            // Idempotent: the work is already done and saying so is the
            // honest answer, not an error.
            return Ok(self.answer(&tenant.id, "already reaped", vec![]));
        }

        let sid = SpaceId(tenant.id.clone());
        let mut did: Vec<serde_json::Value> = Vec::new();

        // ── the attachment, first, so nothing resolves to the database
        // while it is being dropped ──────────────────────────────────────
        let attachment = spacedb::lookup(&self.pool, &sid).await.map_err(|e| {
            ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
        })?;
        match spacedb::detach(&self.pool, &sid).await {
            Ok(true) => did.push(json!({"step": "detach", "outcome": "released"})),
            Ok(false) => did.push(json!({"step": "detach", "outcome": "nothing to release"})),
            Err(e) => did.push(json!({"step": "detach", "outcome": format!("failed: {e}")})),
        }

        // ── the database ────────────────────────────────────────────────
        //
        // Only one this deployment made. A tenant whose custody says the
        // perimeter is theirs has a database the platform did not create,
        // and §4.3 forbids an unattended drop against it.
        match &attachment {
            Some(a) if a.custody == DbCustody::OwnPerimeter => {
                did.push(json!({
                    "step": "database",
                    "outcome": "left standing",
                    "why": "the tenant's own perimeter; the platform did not create it and \
                            does not drop it",
                    "where": format!("{}:{}/{}", a.host, a.port, a.db),
                }));
            }
            Some(a) => {
                let cfg = match ordo_exec::route::target_config(
                    a,
                    self.credentials.as_ref(),
                    &self.template,
                ) {
                    Ok(cfg) => Some(cfg),
                    Err(e) => {
                        did.push(json!({
                            "step": "database",
                            "outcome": format!("not reached: {e}"),
                            "where": format!("{}:{}/{}", a.host, a.port, a.db),
                        }));
                        None
                    }
                };
                if let Some(cfg) = cfg {
                    match ordo_exec::drop_database(&cfg, &a.db).await {
                        Ok(true) => did
                            .push(json!({"step": "database", "outcome": "dropped", "where": a.db})),
                        Ok(false) => did.push(
                            json!({"step": "database", "outcome": "was not there", "where": a.db}),
                        ),
                        Err(e) => did.push(json!({
                            "step": "database",
                            "outcome": format!("failed: {e}"),
                            "where": a.db,
                        })),
                    }
                }
            }
            None => did.push(json!({
                "step": "database",
                "outcome": "no attachment recorded",
                // The registry's own `target` field, which is written
                // before the database is made precisely so this case can
                // still name a place to look.
                "recorded_target": tenant.target,
            })),
        }

        // ── the fleet view's cached row ─────────────────────────────────
        if !tenant.target.is_empty() {
            match ordo_exec::fleet::forget(&self.pool, &tenant.target).await {
                Ok(()) => did.push(json!({"step": "fleet", "outcome": "forgotten"})),
                Err(e) => did.push(json!({"step": "fleet", "outcome": format!("failed: {e}")})),
            }
        }

        // ── custody ─────────────────────────────────────────────────────
        //
        // Last of the resources, and deliberately after the database: keys
        // destroyed before the rows they seal would leave unreadable data
        // standing, which is a worse residue than a database with no keys.
        match self.keys.destroy_space(&sid) {
            Ok(()) => did.push(json!({"step": "custody", "outcome": "destroyed"})),
            Err(e) => did.push(json!({"step": "custody", "outcome": format!("failed: {e}")})),
        }

        // ── the registry row, last ──────────────────────────────────────
        //
        // The row is what names everything above, so it is released only
        // once they are. A step that failed leaves the space `failed` with
        // what happened on it, rather than `reaped`, which would be a
        // claim that nothing is left.
        let clean = did.iter().all(|d| {
            !d["outcome"]
                .as_str()
                .unwrap_or_default()
                .starts_with("failed")
        });
        let at = (self.now)();
        if clean {
            tenants::reaped(&self.pool, &tenant.id, at)
                .await
                .map_err(|e| {
                    ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
                })?;
        } else {
            let note = format!("reap incomplete: {}", serde_json::Value::Array(did.clone()));
            let _ = tenants::fail(&self.pool, &tenant.id, &note, at).await;
        }

        if let Some(stream) = &self.stream {
            stream.observe(
                ordo_secevent::Observation {
                    space: Some(ordo_commandlog::SpaceId(tenant.id.clone())),
                    subject: ordo_secevent::Subject::platform("provisioning"),
                    op: "reap".to_string(),
                    kind: ordo_secevent::EventKind::CustodyAction {
                        action: format!(
                            "space_reaped by={caller} phase={} outcome={}",
                            tenant.phase.as_str(),
                            if clean { "reaped" } else { "incomplete" }
                        ),
                    },
                    at: ordo_commandlog::Timestamp(at),
                    correlation: ordo_secevent::correlation::adopt(&format!("reap_{}", tenant.id)),
                },
                ordo_secevent::Relationship::None,
            );
        }

        Ok(self.answer(&tenant.id, if clean { "reaped" } else { "incomplete" }, did))
    }

    fn answer(&self, id: &str, outcome: &str, did: Vec<serde_json::Value>) -> ExecResult {
        ExecResult {
            position: None,
            rows: vec![json!({ "space": id, "outcome": outcome, "released": did })],
            next_cursor: None,
        }
    }
}
