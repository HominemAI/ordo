//! A tenant's own say in recovery: **hold on recovery** (`plan-passkey-recovery.md`
//! §5.3, R6) and the ways a tenant may make recovery **stricter** for the
//! people it created (§4.4, R7): a longer delay, more approvals, no recovery by
//! its administrators, no recovery codes.
//!
//! **A tenant can only tighten.** A tenant loosening a deployment's floor would
//! let one tenant weaken a credential other tenants rely on, which the owner's
//! principle that credentials belong to the deployment forbids
//! (`corpus-rulings.md`, 2026-09-29, P1 and P6). Every setting is checked
//! against the deployment's own when it is made, and the deployment's is what
//! is used when it is the stricter.
//!
//! A credential belongs to the deployment and reaches every tenant its holder
//! belongs to. So when the deployment recovers a person, the new passkey opens
//! every tenant they are in. A tenant that wants more than that, because its
//! compliance position is that no outside party's judgement admits anybody,
//! can turn on *hold*: a recovered person's membership in that tenant is
//! suspended by the recovery, and stays suspended until an owner or manager of
//! the tenant reactivates it with the ordinary, step-up-gated
//! `std.space.reinstate`.
//!
//! **Off by default** (owner, 2026-09-29: a recovered person is not locked out
//! of each tenant). With it on, a person in three tenants who loses a phone
//! waits on three administrators, which is a bad experience for the common
//! honest case, so it is the tenant's choice and the deployment's default is
//! the deployment owner's setting ([`crate::recovery::RecoveryPolicy::hold_default`]).
//!
//! **Not applied to a tenant's only active owner.** Nobody would be left to
//! release it. That person is the last-owner case, restored by the deployment
//! owner, and the tenant is told.
//!
//! A hold is a [`Settlement`], run when a recovery **completes** and not when
//! it is opened, so a case that is cancelled or contested holds nobody.

use std::sync::Arc;

use ordo_secevent::{EventKind, Observation, Relationship, Subject};

use crate::member_ops::MemberOps;
use crate::recovery::{Settlement, TenantsOf};
use ordo_exec::{sql_lit, Pool, PoolError};

/// One row per tenant that has chosen. A tenant that has not chosen takes the
/// deployment's default.
pub const TENANT_RECOVERY_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$recoverytenant" (
  "space"      text    NOT NULL,
  "hold"       boolean,
  "updated_by" text    NOT NULL DEFAULT '',
  "updated_at" bigint  NOT NULL DEFAULT 0,
  PRIMARY KEY ("space")
)"#;

/// What R7 added, and one change to R6's column: `hold` is null when the
/// tenant has not chosen, so a tenant that changed another setting still takes
/// the deployment's default for it.
pub const TENANT_RECOVERY_ADDED_DDL: [&str; 7] = [
    r#"ALTER TABLE "$recoverytenant" ALTER COLUMN "hold" DROP NOT NULL"#,
    r#"ALTER TABLE "$recoverytenant" ALTER COLUMN "hold" DROP DEFAULT"#,
    r#"ALTER TABLE "$recoverytenant" ADD COLUMN IF NOT EXISTS "delay_secs" bigint NOT NULL DEFAULT 0"#,
    r#"ALTER TABLE "$recoverytenant" ADD COLUMN IF NOT EXISTS "approvals" integer NOT NULL DEFAULT 1"#,
    r#"ALTER TABLE "$recoverytenant" ADD COLUMN IF NOT EXISTS "path_enabled" boolean NOT NULL DEFAULT true"#,
    r#"ALTER TABLE "$recoverytenant" ADD COLUMN IF NOT EXISTS "codes_enabled" boolean NOT NULL DEFAULT true"#,
    r#"ALTER TABLE "$recoverytenant" ADD COLUMN IF NOT EXISTS "managers_enabled" boolean NOT NULL DEFAULT true"#,
];

/// Create `$recoverytenant`, retry-once for the reason the other platform
/// tables do.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    for ddl in [TENANT_RECOVERY_DDL]
        .into_iter()
        .chain(TENANT_RECOVERY_ADDED_DDL)
    {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// What a tenant has chosen. A field the tenant has not set is the
/// deployment's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantSettings {
    /// Whether a recovered person is held here.
    pub hold: bool,
    /// A delay the tenant asks of a case for its own members, on top of the
    /// deployment's.
    pub delay_secs: u64,
    /// The approvals the tenant asks for, at the least.
    pub approvals: u32,
    /// Whether the tenant's administrators may recover the people it created.
    pub path_enabled: bool,
    /// Whether the people it created may use recovery codes.
    pub codes_enabled: bool,
    /// The tenant's own choice about its managers recovering members. True
    /// unless the tenant turned it off. It only narrows: a manager recovers
    /// a member when the deployment allows it **and** this is true.
    pub managers_enabled: bool,
}

/// A change to a tenant's settings: only what is `Some`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Patch {
    pub hold: Option<bool>,
    pub delay_secs: Option<u64>,
    pub approvals: Option<u32>,
    pub path_enabled: Option<bool>,
    pub codes_enabled: Option<bool>,
    pub managers_enabled: Option<bool>,
}

/// What each tenant has chosen about recovery.
#[derive(Clone)]
pub struct TenantRecovery {
    pool: Pool,
    default_hold: bool,
}

impl TenantRecovery {
    pub fn new(pool: Pool, default_hold: bool) -> TenantRecovery {
        TenantRecovery { pool, default_hold }
    }

    /// The tenant's settings: its own where it has chosen, the deployment's
    /// default where it has not.
    pub async fn settings(&self, space: &str) -> Result<TenantSettings, ordo_identity::StoreFault> {
        let rows = self
            .pool
            .rows(&format!(
                r#"SELECT "hold", "delay_secs", "approvals", "path_enabled", "codes_enabled",
                          "managers_enabled"
                   FROM "$recoverytenant" WHERE "space" = {}"#,
                sql_lit(space)
            ))
            .await
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(
                    e,
                    "the tenant recovery table could not be read",
                )
            })?;
        let Some(r) = rows.first() else {
            return Ok(TenantSettings {
                hold: self.default_hold,
                delay_secs: 0,
                approvals: 1,
                path_enabled: true,
                codes_enabled: true,
                managers_enabled: true,
            });
        };
        let text = |i: usize| r.get(i).and_then(|c| c.as_deref());
        let flag = |i: usize| matches!(text(i), Some("t") | Some("true"));
        Ok(TenantSettings {
            hold: match text(0) {
                None => self.default_hold,
                Some(_) => flag(0),
            },
            delay_secs: text(1).and_then(|s| s.parse().ok()).unwrap_or(0),
            approvals: text(2).and_then(|s| s.parse().ok()).unwrap_or(1),
            path_enabled: flag(3),
            codes_enabled: flag(4),
            managers_enabled: flag(5),
        })
    }

    /// Does this tenant hold a recovered person? Its own choice when it has
    /// made one, the deployment's default when it has not.
    pub async fn hold(&self, space: &str) -> Result<bool, ordo_identity::StoreFault> {
        Ok(self.settings(space).await?.hold)
    }

    /// The tenant chooses. Only what the patch names changes. **The caller
    /// checks the patch against the deployment's floors first**; this writes
    /// what it is given.
    pub async fn update(
        &self,
        space: &str,
        patch: &Patch,
        by: &str,
        at: u64,
    ) -> Result<TenantSettings, ordo_identity::StoreFault> {
        let current = self.settings(space).await?;
        // `hold` keeps its "not chosen" state unless the patch chooses.
        let chosen: Option<bool> = match patch.hold {
            Some(h) => Some(h),
            None => {
                let rows = self
                    .pool
                    .rows(&format!(
                        r#"SELECT "hold" FROM "$recoverytenant" WHERE "space" = {}"#,
                        sql_lit(space)
                    ))
                    .await
                    .map_err(|e| {
                        crate::auth::store_failure::pool_fault(
                            e,
                            "the tenant recovery table could not be read",
                        )
                    })?;
                rows.first()
                    .and_then(|r| r.first())
                    .and_then(|c| c.as_deref())
                    .map(|v| matches!(v, "t" | "true"))
            }
        };
        let next = TenantSettings {
            hold: chosen.unwrap_or(current.hold),
            delay_secs: patch.delay_secs.unwrap_or(current.delay_secs),
            approvals: patch.approvals.unwrap_or(current.approvals).max(1),
            path_enabled: patch.path_enabled.unwrap_or(current.path_enabled),
            codes_enabled: patch.codes_enabled.unwrap_or(current.codes_enabled),
            managers_enabled: patch.managers_enabled.unwrap_or(current.managers_enabled),
        };
        self.pool
            .rows(&format!(
                r#"INSERT INTO "$recoverytenant"
                     ("space", "hold", "delay_secs", "approvals", "path_enabled",
                      "codes_enabled", "managers_enabled", "updated_by", "updated_at")
                   VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {at})
                   ON CONFLICT ("space") DO UPDATE SET
                     "hold" = EXCLUDED."hold",
                     "delay_secs" = EXCLUDED."delay_secs",
                     "approvals" = EXCLUDED."approvals",
                     "path_enabled" = EXCLUDED."path_enabled",
                     "codes_enabled" = EXCLUDED."codes_enabled",
                     "managers_enabled" = EXCLUDED."managers_enabled",
                     "updated_by" = EXCLUDED."updated_by",
                     "updated_at" = EXCLUDED."updated_at""#,
                sql_lit(space),
                match chosen {
                    Some(h) => h.to_string(),
                    None => "NULL".to_string(),
                },
                next.delay_secs,
                next.approvals,
                next.path_enabled,
                next.codes_enabled,
                next.managers_enabled,
                sql_lit(by)
            ))
            .await
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(
                    e,
                    "the tenant recovery table could not be written",
                )
            })?;
        Ok(next)
    }

    /// The tenant chooses about hold alone. Kept for the callers that have
    /// nothing else to say.
    pub async fn set_hold(
        &self,
        space: &str,
        hold: bool,
        by: &str,
        at: u64,
    ) -> Result<(), ordo_identity::StoreFault> {
        self.update(
            space,
            &Patch {
                hold: Some(hold),
                ..Patch::default()
            },
            by,
            at,
        )
        .await
        .map(|_| ())
    }
}

/// Suspend a recovered person in every tenant that holds, once the recovery
/// completes.
pub struct HoldOnRecovery {
    pub members: Arc<MemberOps>,
    pub tenants: TenantRecovery,
    /// Which tenants a person belongs to, from the door's engine.
    pub tenants_of: TenantsOf,
    /// The runtime the stores are on. A settlement is synchronous and runs on
    /// a blocking thread, so it drives the async stores through this.
    pub runtime: tokio::runtime::Handle,
}

impl Settlement for HoldOnRecovery {
    fn name(&self) -> &'static str {
        "hold-on-recovery"
    }

    fn settle(&self, principal: &str, case: &str) -> Result<(), ordo_identity::StoreFault> {
        let spaces = (self.tenants_of)(principal);
        self.runtime.block_on(async {
            for space in spaces {
                if !self.tenants.hold(&space).await? {
                    continue;
                }
                let held = self
                    .members
                    .hold_member(&space, principal)
                    .await
                    .map_err(crate::auth::store_failure::exec_fault)?;
                if held.is_none() {
                    // Not active, or the tenant's only active owner.
                    continue;
                }
                if let Some(stream) = &self.members.stream {
                    stream.observe(
                        Observation {
                            space: Some(ordo_commandlog::SpaceId(space.clone())),
                            subject: Subject::platform("recovery-hold"),
                            op: format!("recovery.hold case={case}"),
                            kind: EventKind::MembershipSuspended,
                            at: ordo_commandlog::Timestamp(self.members.at()),
                            correlation: ordo_secevent::correlation::mint(),
                        },
                        Relationship::Member,
                    );
                }
                let _ = self.members.rebuilt(&space).await;
            }
            Ok(())
        })
    }
}

/// Whether a person may use recovery codes: not when the tenant that created
/// them has switched codes off for the people it created. **Fails closed**: a
/// database that cannot be read is an answer of "no", since a code that
/// bypassed a tenant's choice is the thing the choice is for.
pub struct TenantCodesGate {
    pub managed: crate::credential_ops::ManagedStore,
    pub tenants: TenantRecovery,
    pub runtime: tokio::runtime::Handle,
}

impl crate::recovery::CodesGate for TenantCodesGate {
    fn allowed(&self, principal: &str) -> bool {
        self.runtime.block_on(async {
            match self.managed.managed_by(principal).await {
                Ok(None) => true,
                Ok(Some(space)) => self
                    .tenants
                    .settings(&space)
                    .await
                    .map(|s| s.codes_enabled)
                    .unwrap_or(false),
                Err(_) => false,
            }
        })
    }
}
