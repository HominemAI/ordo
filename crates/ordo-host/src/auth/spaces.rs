//! A person's own list of tenants: `GET /auth/spaces`
//! (`plan-provisioning-on-signup.md` P4, `plan-sharing.md` G6).
//!
//! The owner ruled on 2026-09-29 that a person may see the list of their own
//! tenants and nobody else sees it. This is the route a page calls after a
//! sign-in to learn where to go: the tenant the person was given, the ones
//! they were admitted to, and the ones they are a guest in.
//!
//! # Whose list
//!
//! **The token's, and nobody else's.** There is no parameter that names a
//! person. The principal is the one the bearer token proves, and the answer
//! is read from the engine the door decides with, so a membership that was
//! removed or suspended is as absent here as it is at the door.
//!
//! A capability token or a delegated one may not list (owner, 2026-10-25).
//! It is an app's or a window's authority over named ops in one tenant, not
//! a person's, so it is not the person asking; it gets `403 access_denied`,
//! the refusal an op gives that token kind.
//!
//! # `via`
//!
//! `member` for a seat with a role. `guest` for a seat with none, which is
//! what a share accept mints (`plan-sharing.md` §3.1). A guest's `role` is
//! null.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_exec::Pool;
use ordo_identity::{AccessEngine, MembershipStatus, PrincipalId, Timestamp};
use serde_json::{json, Value};

use super::{bearer, refused, unconfigured, SharedAuth};

/// Where the list is read from: the door's live engine, and the control
/// database for a tenant's name.
pub struct TenantLister {
    /// The engine the door decides with, read on every call, because a
    /// program swap or a membership change installs a new one.
    pub engine: Arc<dyn Fn() -> Arc<AccessEngine> + Send + Sync>,
    /// The tenant registry, for names. `None` names nobody.
    pub pool: Option<Pool>,
}

/// One tenant a person belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub space: String,
    pub name: Option<String>,
    pub role: Option<String>,
    pub via: &'static str,
    /// The tenant's registry row, for its pause (27ar). `None` for a tenant
    /// the registry does not hold, which reads as active.
    pub tenant: Option<crate::tenants::Tenant>,
}

impl TenantLister {
    /// The active tenants `principal` belongs to, by tenant id.
    pub async fn list(&self, principal: &PrincipalId) -> Vec<Listed> {
        let engine = (self.engine)();
        let mut held: Vec<_> = engine
            .spaces_of(principal)
            .into_iter()
            .filter(|(_, _, status)| *status == MembershipStatus::Active)
            .collect();
        held.sort_by(|a, b| a.0 .0.cmp(&b.0 .0));
        let mut out = Vec::new();
        for (space, role, _) in held {
            let tenant = match &self.pool {
                Some(pool) => crate::tenants::lookup(pool, &space.0).await.ok().flatten(),
                None => None,
            };
            let name = tenant.as_ref().map(|t| t.name.clone());
            let word = match role {
                Some(ordo_identity::Role::Owner) => Some("owner"),
                Some(ordo_identity::Role::Manager) => Some("manager"),
                Some(ordo_identity::Role::Editor) => Some("editor"),
                Some(ordo_identity::Role::Viewer) => Some("viewer"),
                None => None,
            };
            out.push(Listed {
                name,
                role: word.map(str::to_string),
                via: if role.is_some() { "member" } else { "guest" },
                space: space.0,
                tenant,
            });
        }
        out
    }
}

/// `GET /auth/spaces` with the session's bearer token.
pub async fn list(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(token) = bearer(&headers) else {
        auth.record_refusal("auth.spaces", "no-token", now);
        return refused();
    };
    let Ok(claims) = ordo_identity::verify(&auth.token_key, &token, now) else {
        auth.record_refusal("auth.spaces", "token-unverified", now);
        return refused();
    };
    // A capability or a delegation is not the person asking: only a
    // person's own session lists their tenants (owner, 2026-10-25). The
    // refusal is the one an op gives that token kind.
    if claims.capability.is_some() || claims.delegation.is_some() {
        auth.record_refusal("auth.spaces", "not-a-session", now);
        return ordo_api::ApiError::new(
            ordo_api::ErrorCode::AccessDenied,
            "op is not callable with this token",
        )
        .into_response();
    }
    if let (Some(session), Some(revocations)) = (claims.session.as_ref(), auth.revocations.as_ref())
    {
        if revocations.is_revoked(&session.id)
            || revocations.principal_ended(&claims.principal, session.started_at)
        {
            auth.record_refusal("auth.spaces", "session-ended", now);
            return refused();
        }
    }
    let Some(lister) = auth.spaces() else {
        return (StatusCode::OK, Json(json!([]))).into_response();
    };
    let rows: Vec<Value> = lister
        .list(&claims.principal)
        .await
        .into_iter()
        .map(|l| {
            let mut row = json!({
                "space": l.space,
                "name": l.name,
                "role": l.role,
                "via": l.via,
            });
            crate::pause::list_state(&mut row, l.tenant.as_ref());
            row
        })
        .collect();
    (StatusCode::OK, Json(Value::Array(rows))).into_response()
}
