//! Deleting one's own account: `POST /auth/account/delete` (checklist 27aq
//! R6, `plan-member-release.md` §5, owner rulings of 2026-10-05).
//!
//! # Why a route and not an op
//!
//! An op is bound to a tenant. The account is the deployment's directory,
//! and the person's own routes live under `/auth`.
//!
//! # What it asks for, in order
//!
//! 1. The bearer token is a live session's, not a capability or a delegation.
//! 2. Fresh presence (a passkey step-up), and `{"confirm": "<principal id>"}`.
//! 3. **No membership remains.** The person holds no seat in any tenant, as a
//!    member or a guest, and no tenant of their own. Otherwise the answer is
//!    `409 memberships_remain` with one row per tenant and what stands in the
//!    way: `own`, `last_owner`, `guest`, `locked`, `held` (with the release
//!    rule's conditions as written), or `open` (the rule clears: leave first).
//!    Ordo produces no client app, so this body is the list the owner ruled
//!    the person is given (ask 8).
//!
//! # What it does (owner ruling 2026-10-06)
//!
//! *"account should be marked deleted but not removed ... and then dev can
//! decide if they get removed automatically."*
//!
//! 1. Every session ends.
//! 2. The identity is **marked deleted**. It never signs in again, and its
//!    sign-in identifiers stay linked to it, so none can be used again.
//! 3. `identity-deleted` is recorded on the deployment's stream. The answer
//!    is a receipt naming that record.
//! 4. The person's `left` seats in every tenant become `deleted`, through
//!    the write protocol. The name stays; the app decides how it reads.
//! 5. The data is **kept**, unless the app developer's
//!    `ORDO_ACCOUNT_REMOVAL` says otherwise ([`crate::account_removal`]):
//!    `now` removes it in this request, and under `<n>d` the removal worker
//!    removes it once `deleted_at + n days` has passed, worked out at each pass
//!    from the setting in force.
//!    Removal is [`crate::identity_removal::remove`], recorded as
//!    `identity-removed`. The chain keeps the random `usr_…` id either way;
//!    it is never rewritten.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_api::{ApiError, EdgeCode, EdgeError, ErrorCode};
use ordo_identity::{AccessEngine, MembershipStatus, PrincipalId, Timestamp};
use serde_json::{json, Value};

use super::{refused, unconfigured, SharedAuth};
use crate::release_ops::{Bar, ReleaseOps};

const OP: &str = "auth.account.delete";

/// What the route needs beyond the authentication state.
pub struct Accounts {
    /// Each tenant's release rule, evaluated for the refusal body; and the
    /// control database, which is the membership commands'.
    pub release: Arc<ReleaseOps>,
    /// The engine the door decides with, read on every call.
    pub engine: Arc<dyn Fn() -> Arc<AccessEngine> + Send + Sync>,
    /// The app developer's account removal setting. Default: never.
    pub removal: crate::account_removal::Removal,
}

impl Accounts {
    /// One row per tenant that still holds the person, with what stands in
    /// the way. Empty when nothing does.
    pub async fn standing(&self, principal: &PrincipalId) -> Result<Vec<Value>, String> {
        let control = self.release.members.control.clone();
        let mut held: Vec<_> = (self.engine)().spaces_of(principal);
        held.sort_by(|a, b| a.0 .0.cmp(&b.0 .0));
        let mut rows = Vec::new();
        let mut listed: Vec<String> = Vec::new();
        for (space, role, status) in held {
            let bar = self
                .release
                .bar_for(&space.0, &principal.0)
                .await
                .map_err(|e| e.message)?;
            let Some(bar) = bar else {
                continue;
            };
            let mut row = json!({
                "space": space.0,
                "name": name_of(&control, &space.0).await,
                "role": role.map(role_word),
                "status": match status {
                    MembershipStatus::Active => "active",
                    MembershipStatus::Suspended => "suspended",
                },
                "bar": bar.word(),
            });
            if let Bar::Held(atoms) = &bar {
                row["held"] = json!(atoms);
            }
            listed.push(space.0.clone());
            rows.push(row);
        }
        // A tenant of their own with no seat in it still holds them: it is
        // theirs to delete or hand on.
        if crate::registrations::table_exists(&control, "$spaces").await? {
            if let Some(own) = crate::tenants::own_of(&control, &principal.0)
                .await
                .map_err(|e| e.to_string())?
            {
                if !listed.contains(&own.id) {
                    rows.push(json!({
                        "space": own.id,
                        "name": own.name,
                        "role": null,
                        "status": null,
                        "bar": Bar::Own.word(),
                    }));
                }
            }
        }
        Ok(rows)
    }
}

async fn name_of(pool: &ordo_exec::Pool, space: &str) -> Option<String> {
    crate::tenants::lookup(pool, space)
        .await
        .ok()
        .flatten()
        .map(|t| t.name)
}

fn role_word(r: ordo_identity::Role) -> &'static str {
    match r {
        ordo_identity::Role::Owner => "owner",
        ordo_identity::Role::Manager => "manager",
        ordo_identity::Role::Editor => "editor",
        ordo_identity::Role::Viewer => "viewer",
    }
}

/// A refusal from the edge's own inventories; the status is the code's.
fn edge(code: EdgeCode, message: &str) -> Response {
    EdgeError::new(code, message).into_response()
}

fn op_error(code: ErrorCode, message: &str) -> Response {
    ApiError::new(code, message).into_response()
}

/// `POST /auth/account/delete {"confirm": "<principal id>"}`.
pub async fn delete(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let claims = match super::step_up_claims(&auth, &headers, OP, now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    if !super::recover::fresh_presence(&claims, now) {
        auth.record_refusal(OP, "presence-required", now);
        return edge(
            EdgeCode::PresenceRequired,
            "deleting your account needs a fresh passkey assertion: POST /auth/step-up first",
        );
    }
    let principal = claims.principal.clone();
    if body.get("confirm").and_then(|c| c.as_str()) != Some(principal.0.as_str()) {
        auth.record_refusal(OP, "not-confirmed", now);
        return op_error(
            ErrorCode::NotConfirmed,
            "say which account: `confirm` repeats your principal id",
        );
    }
    let Some(accounts) = auth.account().cloned() else {
        auth.record_refusal(OP, "not-wired", now);
        return refused();
    };
    let standing = match accounts.standing(&principal).await {
        Ok(s) => s,
        Err(_) => {
            auth.record_refusal(OP, "memberships-unreadable", now);
            return op_error(
                ErrorCode::SpaceUnavailable,
                "your memberships could not be read; nothing was deleted",
            );
        }
    };
    if !standing.is_empty() {
        auth.record_refusal(OP, "memberships-remain", now);
        return EdgeError::new(
            EdgeCode::MembershipsRemain,
            "you still belong to these tenants; leave each one first. \
             `bar` says what stands in the way",
        )
        .beside("tenants", json!(standing))
        .into_response();
    }
    let control = accounts.release.members.control.clone();
    if crate::identity_removal::end_sessions(
        auth.revocations.clone(),
        &principal.0,
        now.0,
        auth.session_max_secs,
    )
    .await
    .is_err()
    {
        auth.record_refusal(OP, "session-store-unavailable", now);
        return edge(
            EdgeCode::SessionStoreUnavailable,
            "your sessions could not be ended; nothing was deleted",
        );
    }
    // The mark, through the directory every sign-in asks.
    let marked = {
        let auth2 = auth.clone();
        let who = principal.clone();
        tokio::task::spawn_blocking(move || {
            auth2
                .directory
                .lock()
                .expect("directory mutex poisoned")
                .mark_deleted(&who, now)
        })
        .await
    };
    if !matches!(marked, Ok(Ok(()))) {
        auth.record_refusal(OP, "directory-unavailable", now);
        return op_error(
            ErrorCode::SpaceUnavailable,
            "your sessions ended and the account could not be marked deleted; ask again",
        );
    }
    let correlation = crate::identity_removal::record(
        auth.recorder.as_deref(),
        "identity-deleted",
        &principal.0,
        "self",
        None,
        &principal.0,
        now.0,
    );
    // The seats first, while the person can still be named: each left seat
    // becomes `deleted`. A tenant that could not be reached is named in the
    // answer and on the record; the account is deleted either way.
    let seats = crate::account_removal::mark_seats(&accounts.release, &principal.0).await;
    if !seats.unreached.is_empty() {
        auth.record_refusal(OP, "seats-unreached", now);
    }
    let removal = accounts.removal;
    let due = removal.due(now.0);
    let mut removed = false;
    match removal {
        crate::account_removal::Removal::Never => {}
        crate::account_removal::Removal::Now => {
            if crate::account_removal::remove_now(
                &control,
                auth.recorder.as_deref(),
                &principal.0,
                "self",
                now.0,
            )
            .await
            .is_err()
            {
                auth.record_refusal(OP, "directory-unavailable", now);
                return op_error(
                    ErrorCode::SpaceUnavailable,
                    "your account is deleted and its data could not be removed; ask the \
                     deployment owner",
                );
            }
            removed = true;
        }
        // The worker works the due time out from `deleted_at` at each pass.
        crate::account_removal::Removal::AfterDays(_) => {}
    }
    (
        StatusCode::OK,
        Json(json!({
            "deleted": principal.0,
            "at": now.0,
            "removal": {
                "setting": removal.word(),
                "removeAt": due,
                "removed": removed,
            },
            "seats": {
                "marked": seats.marked,
                "unreached": seats.unreached,
            },
            "receipt": {
                "stream": "deployment",
                "act": "identity-deleted",
                "reason": "self",
                "correlation": correlation,
            },
        })),
    )
        .into_response()
}
