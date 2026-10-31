//! What deleting a tenant does to identities (`plan-enrolment.md` §4.6, E7,
//! owner ruling A5).
//!
//! `std.space.delete` ends every membership in the tenant, because the rows
//! go. The identities are the deployment's (P1) and outlive it, so what
//! becomes of the ones the tenant made is a separate act, and it is this.
//!
//! | The person | What happens |
//! |---|---|
//! | a managed identity that has signed in | released: independent from now on, on the record. Credential, links and passkeys stay |
//! | a managed identity that never signed in | removed, with its credential, links, passkey rows and unused tickets |
//! | a person who arrived through the tenant's own provider | the provider's links go with the tenant. A principal left with no link is removed, and its sessions end |
//! | the same person, holding a platform sign-in as well | the platform sign-in remains, and what it reaches is the app developer's decision: nothing, until some tenant admits them |
//!
//! # "Ever signed in"
//!
//! It is `$signedin`, written by [`ordo_identity::DirectoryStore::note_sign_in`]
//! from `ordo_identity::sign_in`, the one function every completed sign-in
//! passes through. That is the place a session is minted from, and it is
//! durable and synchronous, which the security event stream is not: the
//! stream collapses and can lose entries under load, and a person kept or
//! removed by whether an event survived would be decided by luck.
//!
//! # The tenant's own provider
//!
//! A tenant registering its own identity provider is
//! `plan-auth-federated.md` F12 and is not built. [`TenantConnections`] is
//! the seam it will fill: which connections a tenant owns, and retiring one.
//! [`NoConnections`] owns none, which is the truth today.
//!
//! # Every act is recorded
//!
//! At deployment scope, as `identity-released` or `identity-removed`, naming
//! the reason, the person and the owner who deleted the tenant. The tenant's
//! own stream is not written to: the tenant is gone.

use std::sync::Arc;

use futures::future::BoxFuture;
use ordo_exec::{sql_lit, Pool};
use ordo_identity::session::SessionRevocations;
use ordo_secevent::Recorder;

use crate::registrations::table_exists;

/// The provider connections a tenant registered for itself. Built by F12;
/// until then [`NoConnections`].
pub trait TenantConnections: Send + Sync {
    /// The connection names this tenant owns. A connection `acme` names its
    /// links `oidc:acme:<subject>` and `saml:acme:<name-id>`.
    fn owned_by<'a>(&'a self, tenant: &'a str) -> BoxFuture<'a, Result<Vec<String>, String>>;
    /// Retire one, so its name is never served again.
    fn retire<'a>(
        &'a self,
        tenant: &'a str,
        connection: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;
}

/// No tenant owns a connection, because none can register one yet.
pub struct NoConnections;

impl TenantConnections for NoConnections {
    fn owned_by<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Vec<String>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn retire<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

/// What one deletion did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Managed identities that became independent.
    pub released: Vec<String>,
    /// Identities destroyed, whichever the reason.
    pub removed: Vec<String>,
    /// The tenant's provider connections that were retired.
    pub retired: Vec<String>,
}

pub struct TenantIdentities {
    /// The control database, where identities live.
    pub control: Pool,
    pub connections: Arc<dyn TenantConnections>,
    /// Ends the sessions of an identity that is destroyed while it may still
    /// hold some. `None` in a deployment with no session deny list.
    pub revocations: Option<Arc<dyn SessionRevocations>>,
    /// `ORDO_SESSION_MAX_SECS`: how long an ended-sessions mark is kept.
    pub session_max_secs: u64,
    pub stream: Option<Arc<Recorder>>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl TenantIdentities {
    /// Settle every identity the deleted tenant `tenant` was responsible
    /// for. `by` is the owner who deleted it. Idempotent: what a pass did is
    /// not done again by the next.
    pub async fn after_delete(&self, tenant: &str, by: &str) -> Result<Outcome, String> {
        let at = (self.now)();
        let mut out = Outcome::default();

        // The tenant's own providers first, so a managed person who also
        // arrived through one is settled once, as the provider's.
        for connection in self.connections.owned_by(tenant).await? {
            let mut people: Vec<String> = Vec::new();
            for kind in ["oidc", "saml", "oauth2"] {
                let prefix = format!("{kind}:{connection}:");
                let rows = self
                    .control
                    .rows(&format!(
                        r#"DELETE FROM "$authenticator"
                           WHERE left("authenticator", {}) = {}
                           RETURNING "principal""#,
                        prefix.chars().count(),
                        sql_lit(&prefix)
                    ))
                    .await
                    .map_err(|e| e.to_string())?;
                people.extend(rows.into_iter().filter_map(|r| r[0].clone()));
            }
            people.sort();
            people.dedup();
            for person in people {
                let left = self
                    .control
                    .rows(&format!(
                        r#"SELECT count(*) FROM "$authenticator" WHERE "principal" = {}"#,
                        sql_lit(&person)
                    ))
                    .await
                    .map_err(|e| e.to_string())?;
                let none_left = left
                    .first()
                    .and_then(|r| r[0].as_deref())
                    .is_some_and(|n| n == "0");
                if none_left {
                    self.end_sessions(&person, at).await?;
                    crate::identity_removal::remove(&self.control, &person).await?;
                    self.record(
                        "identity-removed",
                        &person,
                        "tenant-provider",
                        tenant,
                        by,
                        at,
                    );
                    out.removed.push(person);
                }
            }
            self.connections.retire(tenant, &connection).await?;
            out.retired.push(connection);
        }

        // The people the tenant made.
        if table_exists(&self.control, "$managed").await? {
            let has_signedin = table_exists(&self.control, "$signedin").await?;
            let rows = self
                .control
                .rows(&format!(
                    r#"SELECT "principal" FROM "$managed"
                       WHERE "tenant" = {} AND "released_at" IS NULL ORDER BY "principal""#,
                    sql_lit(tenant)
                ))
                .await
                .map_err(|e| e.to_string())?;
            for person in rows.into_iter().filter_map(|r| r[0].clone()) {
                let signed_in = has_signedin
                    && !self
                        .control
                        .rows(&format!(
                            r#"SELECT 1 FROM "$signedin" WHERE "principal" = {}"#,
                            sql_lit(&person)
                        ))
                        .await
                        .map_err(|e| e.to_string())?
                        .is_empty();
                if signed_in {
                    self.control
                        .rows(&format!(
                            r#"UPDATE "$managed" SET "released_at" = {at}
                               WHERE "principal" = {} AND "released_at" IS NULL"#,
                            sql_lit(&person)
                        ))
                        .await
                        .map_err(|e| e.to_string())?;
                    self.record(
                        "identity-released",
                        &person,
                        "tenant-deleted",
                        tenant,
                        by,
                        at,
                    );
                    out.released.push(person);
                } else {
                    crate::identity_removal::remove(&self.control, &person).await?;
                    self.record(
                        "identity-removed",
                        &person,
                        "never-signed-in",
                        tenant,
                        by,
                        at,
                    );
                    out.removed.push(person);
                }
            }
        }
        Ok(out)
    }

    async fn end_sessions(&self, person: &str, at: u64) -> Result<(), String> {
        crate::identity_removal::end_sessions(
            self.revocations.clone(),
            person,
            at,
            self.session_max_secs,
        )
        .await
    }

    fn record(&self, act: &str, person: &str, reason: &str, tenant: &str, by: &str, at: u64) {
        crate::identity_removal::record(
            self.stream.as_deref(),
            act,
            person,
            reason,
            Some(tenant),
            by,
            at,
        );
    }
}
