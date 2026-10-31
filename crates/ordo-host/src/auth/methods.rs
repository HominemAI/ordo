//! `GET /auth/methods`: the ways in this deployment serves, so a page builds
//! its sign-in screen from the door instead of from a build variable.
//!
//! Public, no token, and it says only what a sign-in button already shows
//! (owner, 2026-10-10, `signin-methods-route`):
//!
//! ```text
//! { passkey, password, email, guest,
//!   humanCheck: { kind: "turnstile", siteKey? } | null,
//!   connections: [{ name, label, kind: "oauth2" | "oidc", tenant? }] }
//! ```
//!
//! Never a secret, a client id, a secret file, an issuer or an endpoint. The
//! `label` is a display name: a well-known provider's own for one of the
//! deployment's connections, otherwise the connection's name as written. A
//! tenant's connection is always labelled by its name as written, so a tenant
//! that names its connection `google` is not shown as Google (audit
//! 2026-10-11, finding 30).
//!
//! The answer may be kept for `ORDO_AUTH_METHODS_MAX_AGE` seconds (default
//! 60), by the browser and by a shared cache. A retired tenant connection can
//! show on a page that long; it refuses new sign-ins at once either way.
//!
//! `?tenant=<id>` adds that tenant's own active providers, each marked
//! `tenant: true`. A tenant that has none, one that does not exist and a
//! malformed id all answer the same shape with nothing added, so the route
//! cannot be used to list tenants.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use super::{guest::GuestMode, SharedAuth};

/// `ORDO_AUTH_METHODS_MAX_AGE`: how long a page or a shared cache may keep
/// the answer, in seconds.
pub const MAX_AGE_ENV: &str = "ORDO_AUTH_METHODS_MAX_AGE";
pub const DEFAULT_MAX_AGE: u64 = 60;
/// The longest a deployment may set: an hour.
pub const MAX_MAX_AGE: u64 = 3600;

/// How a well-known provider is named on a button. Any other name is shown
/// as written.
fn label(name: &str) -> String {
    match name {
        "discord" => "Discord",
        "github" => "GitHub",
        "google" => "Google",
        "microsoft" => "Microsoft",
        other => other,
    }
    .to_string()
}

fn entry(name: &str, kind: &str, tenant: bool) -> Value {
    if tenant {
        // As written: a well-known label would vouch for an issuer the
        // tenant chose.
        json!({"name": name, "label": name, "kind": kind, "tenant": true})
    } else {
        json!({"name": name, "label": label(name), "kind": kind})
    }
}

pub async fn list(
    State(auth): State<Option<SharedAuth>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let max_age = auth.as_ref().map_or(DEFAULT_MAX_AGE, |a| a.methods_max_age);
    let mut body = json!({
        "passkey": false, "password": false, "email": false, "guest": false,
        "humanCheck": null, "connections": [],
    });
    if let Some(auth) = auth {
        body["passkey"] = json!(auth.passkey.is_some());
        body["password"] = json!(auth.registry.get("password").is_ok());
        body["email"] = json!(auth.email.is_some());
        body["guest"] = json!(match auth.guests() {
            Some(g) => matches!(g.mode().await, Ok(GuestMode::On)),
            None => false,
        });
        if let Some(check) = &auth.human {
            let mut human = json!({"kind": "turnstile"});
            if let Some(key) = check.site_key() {
                human["siteKey"] = json!(key);
            }
            body["humanCheck"] = human;
        }
        let mut connections: Vec<Value> = Vec::new();
        for name in auth.oauth2.keys() {
            connections.push(entry(name, "oauth2", false));
        }
        for name in auth.oidc.keys() {
            connections.push(entry(name, "oidc", false));
        }
        connections.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        if let (Some(tenant), Some(tc)) = (q.get("tenant"), auth.tenant_connections()) {
            for (name, kind) in tc.public_list(tenant).await {
                connections.push(entry(&name, kind.as_str(), true));
            }
        }
        body["connections"] = Value::Array(connections);
    }
    (
        StatusCode::OK,
        [
            (header::CACHE_CONTROL, format!("public, max-age={max_age}")),
            (header::CONTENT_TYPE, "application/json".to_string()),
        ],
        body.to_string(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tenant_connection_is_labelled_as_written() {
        assert_eq!(entry("google", "oidc", false)["label"], "Google");
        let t = entry("google", "oidc", true);
        assert_eq!(t["label"], "google");
        assert_eq!(t["tenant"], true);
    }
}
