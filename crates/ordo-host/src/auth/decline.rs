//! Declining a ticket: an invitation to a tenant, or a share
//! (owner ruling 2026-10-05, `person-can-leave` ask 1).
//!
//! | Route | Token | Answers |
//! |---|---|---|
//! | `POST /auth/invitation/peek {ticket}` | none | the tenant, the role and the expiry, without spending it |
//! | `POST /auth/invitation/decline {ticket}` | session, when the ticket names a person | the ticket spent as declined |
//! | `POST /auth/share/decline {ticket}` | session, when the ticket names a person | the ticket spent as declined |
//!
//! # What a decline does
//!
//! It spends the ticket, so nobody can redeem it afterwards, and keeps that
//! it was declined. The act is written to the deployment's security stream
//! and mirrored to the tenant's (`invitation-declined`, `share-declined`),
//! named by the same id `std.space.invitations` shows. The invitation
//! listing shows a declined invitation until the instant it would have
//! expired, so the inviter can see it was refused rather than used.
//!
//! # Who may decline
//!
//! A ticket that names nobody is for whoever holds it, and whoever holds it
//! could accept it, so the holder may decline it with no token. A ticket
//! that names a person is that person's alone: a decline needs their
//! session, and a holder who is somebody else is refused and the ticket
//! stays whole, as an accept by the wrong holder is refused
//! ([`crate::tickets::Want`]).
//!
//! # One answer
//!
//! An unknown, spent, expired, mis-addressed or wrong-purpose ticket is one
//! refusal, as it is on the peek and the accept.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_api::{EdgeCode, EdgeError};
use ordo_identity::Timestamp;
use serde_json::{json, Value};

use super::{bearer, step_up_claims, unconfigured, AuthState, SharedAuth};
use crate::tickets::{id_of, Purpose, TicketError, TicketRecord, Tickets, Want};

fn refused(code: EdgeCode) -> Response {
    EdgeError::new(code, "that ticket cannot be used").into_response()
}

fn unavailable(failure: super::StoreFailure) -> Response {
    failure.answer(
        ordo_api::EdgeCode::TicketsUnavailable,
        "tickets cannot be checked right now",
    )
}

/// The words each purpose uses.
struct Kind {
    purpose: Purpose,
    /// The refusal code, the same one the purpose's other routes answer.
    refusal: EdgeCode,
    /// The op name refusals are recorded under.
    op: &'static str,
    /// The act written to the security stream.
    act: &'static str,
}

const INVITATION: Kind = Kind {
    purpose: Purpose::Invitation,
    refusal: EdgeCode::InvitationRefused,
    op: "auth.invitation.decline",
    act: "invitation-declined",
};

const SHARE: Kind = Kind {
    purpose: Purpose::Share,
    refusal: EdgeCode::ShareRefused,
    op: "auth.share.decline",
    act: "share-declined",
};

fn ticket_of(body: &Value) -> Option<&str> {
    body.get("ticket")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn invitation_tickets(auth: &AuthState) -> Option<std::sync::Arc<Tickets>> {
    auth.enrolment().and_then(|e| e.tickets.clone())
}

fn share_tickets(auth: &AuthState) -> Option<std::sync::Arc<Tickets>> {
    auth.sharing().map(|s| s.tickets.clone())
}

/// `POST /auth/invitation/peek {"ticket"}`: what an invitation offers,
/// without spending it. No token, as the share peek: the page shows this
/// before the person has signed in.
pub async fn invitation_peek(
    State(auth): State<Option<SharedAuth>>,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let op = "auth.invitation.peek";
    let Some(ticket) = ticket_of(&body) else {
        auth.record_refusal(op, "malformed-request", now);
        return refused(INVITATION.refusal);
    };
    let Some(tickets) = invitation_tickets(&auth) else {
        auth.record_refusal(op, "tickets-not-wired", now);
        return refused(INVITATION.refusal);
    };
    let want = Want {
        purpose: Some(Purpose::Invitation),
        ..Default::default()
    };
    match tickets.peek(ticket, &want) {
        Ok(rec) => (
            StatusCode::OK,
            Json(json!({
                "space": rec.space,
                "role": rec.role,
                "ticket_expires_at": rec.expires_at,
                "named": rec.principal.is_some(),
            })),
        )
            .into_response(),
        Err(TicketError::Refused) => {
            auth.record_refusal(op, "ticket-refused", now);
            refused(INVITATION.refusal)
        }
        Err(TicketError::Unavailable(why)) => {
            auth.record_refusal(op, "ticket-store-unavailable", now);
            unavailable(why.into())
        }
    }
}

/// `POST /auth/invitation/decline {"ticket"}`.
pub async fn invitation_decline(
    State(auth): State<Option<SharedAuth>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let tickets = invitation_tickets(&auth);
    decline(&auth, &INVITATION, tickets, &headers, &body)
}

/// `POST /auth/share/decline {"ticket"}`.
pub async fn share_decline(
    State(auth): State<Option<SharedAuth>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let tickets = share_tickets(&auth);
    decline(&auth, &SHARE, tickets, &headers, &body)
}

fn decline(
    auth: &AuthState,
    kind: &Kind,
    tickets: Option<std::sync::Arc<Tickets>>,
    headers: &HeaderMap,
    body: &Value,
) -> Response {
    let now = Timestamp((auth.now)());
    let Some(ticket) = ticket_of(body) else {
        auth.record_refusal(kind.op, "malformed-request", now);
        return refused(kind.refusal);
    };
    let Some(tickets) = tickets else {
        auth.record_refusal(kind.op, "tickets-not-wired", now);
        return refused(kind.refusal);
    };
    // A token is optional, and one that is presented must be a live session:
    // a capability or a delegation is not the person.
    let holder = if bearer(headers).is_some() {
        match step_up_claims(auth, headers, kind.op, now) {
            Ok(c) => Some(c.principal.0),
            Err(r) => return *r,
        }
    } else {
        None
    };
    let looked = Want {
        purpose: Some(kind.purpose),
        ..Default::default()
    };
    let rec = match tickets.peek(ticket, &looked) {
        Ok(r) => r,
        Err(e) => return answer(auth, kind, e, now),
    };
    // A ticket that names a person is declined by that person alone.
    if let Some(named) = &rec.principal {
        if holder.as_deref() != Some(named.0.as_str()) {
            auth.record_refusal(kind.op, "not-the-named-holder", now);
            return refused(kind.refusal);
        }
    }
    let want = Want {
        purpose: Some(kind.purpose),
        space: None,
        holder: holder.as_deref(),
    };
    let rec = match tickets.decline(ticket, &want) {
        Ok(r) => r,
        Err(e) => return answer(auth, kind, e, now),
    };
    record(auth, kind, &rec, holder.as_deref(), now);
    (
        StatusCode::OK,
        Json(json!({
            "space": rec.space,
            "outcome": "declined",
        })),
    )
        .into_response()
}

fn answer(auth: &AuthState, kind: &Kind, e: TicketError, now: Timestamp) -> Response {
    match e {
        TicketError::Refused => {
            auth.record_refusal(kind.op, "ticket-refused", now);
            refused(kind.refusal)
        }
        TicketError::Unavailable(why) => {
            auth.record_refusal(kind.op, "ticket-store-unavailable", now);
            unavailable(why.into())
        }
    }
}

/// The decline, in the deployment's stream and the tenant's. Named by the id
/// the invitation listing shows, and by who issued it, so the inviter can
/// find their own.
fn record(auth: &AuthState, kind: &Kind, rec: &TicketRecord, by: Option<&str>, now: Timestamp) {
    let Some(stream) = &auth.recorder else {
        return;
    };
    crate::enrolment::record(
        stream,
        &crate::enrolment::Act {
            act: kind.act,
            detail: format!(
                "id={} issued_by={} principal={}",
                id_of(rec),
                rec.issued_by,
                by.unwrap_or("-"),
            ),
            by,
            tenant: rec.space.as_deref(),
            deployment: true,
            at: now.0,
        },
    );
}
