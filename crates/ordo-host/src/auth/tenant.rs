//! Sign-in through a tenant's own provider (`plan-tenant-providers.md` §5,
//! §6): the `tenant` arm of `begin` and `complete`, and the provider's
//! return leg, `GET /auth/callback/t/{tenant}/{connection}`.
//!
//! The connection is read from the tenant's own database on every call
//! ([`crate::tenant_providers::TenantConnections::resolve`]), so a retired
//! one stops the next sign-in. What happens after the provider answered is
//! `/auth/complete`'s, with three differences: the arrival is at the tenant,
//! at the connection's role; a `space` naming another tenant is the one
//! refusal; and built-in provisioning gives nobody a tenant of their own.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_api::{EdgeCode, EdgeError};
use ordo_identity::{BeginRequest, ChallengeOrRedirect, Timestamp};
use serde_json::{json, Value};

use super::{auth_error_reason, refused, unconfigured, AuthState, SharedAuth, StoreFailure};
use crate::tenant_providers::{Live, Resolved, TenantConnections, SETTING};

impl AuthState {
    /// Give the routes the tenants' connections. Once.
    pub fn wire_tenant_connections(&self, tc: Arc<TenantConnections>) {
        let _ = self.tenant_connections.set(tc);
    }

    pub fn tenant_connections(&self) -> Option<&Arc<TenantConnections>> {
        self.tenant_connections.get()
    }

    /// The connections, when the app developer switched them on.
    fn tenant_providers_on(&self) -> Option<Arc<TenantConnections>> {
        self.tenant_connections().filter(|t| t.enabled).cloned()
    }
}

/// `400 tenant_providers_off`: decided before any identity is known, so it
/// names the setting and the value that changes it.
fn off() -> Response {
    EdgeError::new(
        EdgeCode::TenantProvidersOff,
        crate::tenant_providers::off_message(),
    )
    .with("setting", SETTING)
    .into_response()
}

/// `503 identity_provider_unavailable` for a tenant's connection. No
/// deployment setting fixes a tenant's address, so none is named; the
/// message says what the tenant's owner can change.
fn unavailable(auth: &AuthState, connection: &str, why: &str, now: Timestamp) -> Response {
    auth.record_refusal(
        "auth.begin",
        &format!("identity-provider-unavailable:{why}"),
        now,
    );
    let mut message = format!(
        "the identity provider for this tenant's connection `{connection}` cannot be reached \
         right now"
    );
    match why {
        "denied_address" => message.push_str(
            "; its address resolves to one no tenant's provider may use: loopback, \
             link-local, a cloud metadata address, multicast, reserved space or the \
             deployment's own network. The tenant's owner adds a connection at another address",
        ),
        "discovery-issuer-mismatch" => message.push_str(
            "; its discovery document names another issuer. The tenant's owner adds a \
             connection with exactly the `issuer` it publishes",
        ),
        _ => {}
    }
    EdgeError::new(EdgeCode::IdentityProviderUnavailable, message)
        .with("connection", connection)
        .into_response()
}

/// Look up `tenant`'s connection `connection`. The answer when it cannot be
/// used is the caller's to give.
async fn resolve(
    auth: &AuthState,
    op: &str,
    tenant: &str,
    connection: &str,
    now: Timestamp,
) -> Result<Resolved, Box<Response>> {
    let Some(tc) = auth.tenant_providers_on() else {
        auth.record_refusal(op, "tenant-providers-off", now);
        return Err(Box::new(off()));
    };
    match tc.resolve(tenant, connection).await {
        Ok(Some(r)) => Ok(r),
        Ok(None) => {
            auth.record_refusal(op, "unknown-connection", now);
            Err(Box::new(refused()))
        }
        Err(e) => {
            auth.record_refusal(op, "tenant-connection-unavailable", now);
            Err(Box::new(StoreFailure::from(e).into_response()))
        }
    }
}

/// `POST /auth/begin` with `tenant`.
pub(super) async fn begin(
    auth: &SharedAuth,
    tenant: &str,
    method: &str,
    body: &Value,
    now: Timestamp,
) -> Response {
    let resolved = match resolve(auth, "auth.begin", tenant, method, now).await {
        Ok(r) => r,
        Err(answer) => return *answer,
    };
    let wants_presence = body
        .get("presence")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // Only an OIDC connection carries a two-step claim. Whether it counts
    // is the tenant's `presenceFrom`, read when it is used.
    if wants_presence && matches!(resolved.live, Live::OAuth2(_)) {
        auth.record_refusal("auth.begin", "presence-not-available", now);
        return super::oidc::presence_not_available(method);
    }
    if let Err(refusal) = auth.check_human(body, now).await {
        return *refusal;
    }
    let text = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let req = BeginRequest {
        identifier: text("identifier"),
        return_to: text("return_to"),
        presence: wants_presence,
    };
    let provider = resolved.live.provider();
    let begun = tokio::task::spawn_blocking(move || provider.begin(&req, now)).await;
    match begun {
        Ok(Ok(ChallengeOrRedirect::Redirect { url, binder })) => (
            StatusCode::OK,
            Json(json!({"redirect": url, "binder": binder})),
        )
            .into_response(),
        Ok(Ok(ChallengeOrRedirect::Challenge { .. })) => {
            auth.record_refusal("auth.begin", "unexpected-challenge", now);
            refused()
        }
        Ok(Err(e)) => match e {
            ordo_identity::AuthError::StoreUnavailable(_) => {
                auth.record_refusal("auth.begin", auth_error_reason(&e), now);
                super::ceremony_refusal(&e)
            }
            ordo_identity::AuthError::UpstreamUnavailable(why) => {
                unavailable(auth, method, why, now)
            }
            _ => {
                auth.record_refusal("auth.begin", auth_error_reason(&e), now);
                refused()
            }
        },
        Err(_) => {
            auth.record_refusal("auth.begin", "ceremony-task-failed", now);
            refused()
        }
    }
}

/// `GET /auth/callback/t/{tenant}/{connection}`: a tenant's provider's
/// redirect URI. The same contract as the deployment's
/// ([`super::federated::callback`]).
pub async fn callback(
    State(auth): State<Option<SharedAuth>>,
    Path((tenant, connection)): Path<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let resolved = match resolve(&auth, "auth.callback", &tenant, &connection, now).await {
        Ok(r) => r,
        // The browser came back from a provider: there is no page to
        // name a setting to, and no ceremony to send it back to.
        Err(_) => return refused(),
    };
    match resolved.live {
        Live::Oidc(p) => super::oidc::callback(auth, p, q, now).await,
        Live::OAuth2(p) => super::federated::oauth2_callback(auth, p, q, now).await,
    }
}

/// `POST /auth/complete` with `tenant`: the connection, resolved, with the
/// body's `space` set to the tenant. `Ok(None)` when the body names no
/// tenant. A `space` naming another tenant is the one refusal, before any
/// ceremony is redeemed or anybody minted.
pub(super) async fn for_complete(
    auth: &SharedAuth,
    body: &mut Value,
    method: &str,
    now: Timestamp,
) -> Result<Option<Resolved>, Box<Response>> {
    let Some(tenant) = body
        .get("tenant")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(None);
    };
    if let Some(space) = body.get("space").and_then(Value::as_str) {
        if space != tenant {
            auth.record_refusal("auth.complete", "tenant-connection-other-space", now);
            return Err(Box::new(refused()));
        }
    }
    let resolved = resolve(auth, "auth.complete", &tenant, method, now).await?;
    body["space"] = json!(tenant);
    Ok(Some(resolved))
}

/// The provider of a ceremony that names `tenant`, for linking by claim
/// ([`super::claim`]). `Ok(None)` when the body names no tenant. A sign-in
/// through a tenant's provider is a sign-in like any other: it may open a
/// claim window (to add a passkey, say) and be claimed. What it signs in to
/// is still the person's own tenants and nothing more.
pub(super) async fn claim_provider(
    auth: &AuthState,
    op: &str,
    body: &Value,
    method: &str,
    now: Timestamp,
) -> Result<Option<Arc<dyn ordo_identity::AuthProvider>>, Box<Response>> {
    let Some(tenant) = body.get("tenant").and_then(Value::as_str) else {
        return Ok(None);
    };
    let resolved = resolve(auth, op, tenant, method, now).await?;
    Ok(Some(resolved.live.provider()))
}

/// The arrival of a person who signed in through `resolved`.
pub(super) async fn arrive(auth: &AuthState, resolved: &Resolved, principal: &str) -> &'static str {
    match auth.tenant_connections() {
        Some(tc) => tc.arrive(principal, resolved).await,
        None => "refused",
    }
}

/// May `principal` unlink `authenticator`? Not when it is the last sign-in
/// outside a tenant's own providers held by the last owner who has one, in
/// a tenant with an active connection (`plan-tenant-providers.md` §8). The
/// person is proven, so they are told why.
///
/// On yes, the answer is the locks of every tenant the person owns, held so
/// no other change to those tenants' owners lands between this check and
/// the unlink. The caller hands them to [`release`] once the unlink is done.
pub(super) async fn unlink_guard(
    auth: &AuthState,
    principal: &ordo_identity::PrincipalId,
    authenticator: &ordo_identity::AuthenticatorId,
    now: Timestamp,
) -> Result<Vec<ordo_exec::TxConn>, Box<Response>> {
    let Some(tc) = auth.tenant_providers_on() else {
        return Ok(Vec::new());
    };
    let Some(lister) = auth.spaces() else {
        return Ok(Vec::new());
    };
    // In tenant id order, so two people taking several locks take them in
    // the same order.
    let mut owned: Vec<String> = lister
        .list(principal)
        .await
        .into_iter()
        .filter(|l| l.role.as_deref() == Some("owner"))
        .map(|l| l.space)
        .collect();
    owned.sort();
    owned.dedup();
    let mut held = Vec::new();
    for space in owned {
        let without = crate::tenant_providers::Without::Authenticator(&principal.0, authenticator);
        match tc.hold_if_kept(&space, without).await {
            Ok(Some(tx)) => held.push(tx),
            Ok(None) => {
                TenantConnections::release(held).await;
                auth.record_refusal("auth.authenticators.unlink", "originating-admin", now);
                return Err(Box::new(
                    EdgeError::new(
                        EdgeCode::OriginatingAdmin,
                        crate::tenant_providers::no_originating_admin().message,
                    )
                    .with("space", space)
                    .into_response(),
                ));
            }
            Err(e) => {
                TenantConnections::release(held).await;
                auth.record_refusal("auth.authenticators.unlink", "tenant-unavailable", now);
                return Err(Box::new(StoreFailure::from(e).into_response()));
            }
        }
    }
    Ok(held)
}

/// Let go of what [`unlink_guard`] held.
pub(super) async fn release(held: Vec<ordo_exec::TxConn>) {
    TenantConnections::release(held).await;
}
