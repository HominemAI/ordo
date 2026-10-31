//! One type for a store that did not answer a sign-in route, and one answer
//! for it (owner ruling 2026-10-23: busy versus broken "through some
//! interface rather than individual spots").
//!
//! Every store the sign-in paths read (directory, ceremonies, credentials,
//! passkeys, tickets, recovery, sessions, the mail outbox) fails with an
//! error of its own type. Each converts into [`StoreFailure`], which keeps
//! the one fact the caller may learn: was the deployment **busy** (every
//! pool connection in use, nothing ran, send it again) or is the store
//! **unavailable** (a missing table, a dead database)?
//!
//! The class is the database client's, typed: `ordo_exec::PoolError::class`
//! decides it once, every store carries it in an `ordo_identity::StoreFault`
//! (with the pool error as `source()` when it is safe to keep), and
//! [`IntoFault`] is the one conversion from a store's error to that fault.
//! Nothing here reads an error's words (fix-order step 2,
//! `docs/audit-service-interfaces.md`).
//! A per-flow error
//! enum carries a `StoreFailure` where it once carried a bare word, and the
//! route answers it with [`StoreFailure::into_response`]:
//!
//! - `Busy` is `503 server_busy` with `Retry-After`, the same typed,
//!   retryable answer the ops give (`ordo_api::ApiError::server_busy`).
//! - `Unavailable` is `503 credential_store_unavailable`.
//!
//! Neither depends on the identifier offered, so neither is an oracle.
//! `tests/auth_store_failure_scan.rs` holds every sign-in source to this:
//! no route builds the 503 itself, and no store error is dropped on the
//! way to it.

use std::fmt;

use axum::response::{IntoResponse, Response};
use ordo_api::{EdgeCode, EdgeError};

/// Why a store did not answer a sign-in route. See the module doc.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreFailure {
    /// Every connection the deployment holds was in use. Nothing ran.
    Busy,
    /// Anything else. The text is for the owner's log and never the caller.
    Unavailable(String),
}

impl StoreFailure {
    /// A typed store fault, by its class.
    pub fn from_fault(f: &ordo_identity::StoreFault, text: String) -> StoreFailure {
        if f.is_busy() {
            StoreFailure::Busy
        } else {
            StoreFailure::Unavailable(text)
        }
    }

    /// The store failure inside a ceremony's error, if it is one: a store
    /// that could not be read, or a mail outbox refused for a full pool.
    /// Anything else is a refusal, and `None`.
    pub fn from_auth(e: &ordo_identity::AuthError) -> Option<StoreFailure> {
        match e {
            ordo_identity::AuthError::StoreUnavailable(c) => Some(c.clone().into()),
            ordo_identity::AuthError::UpstreamUnavailable(crate::email_code::MAIL_OUTBOX_BUSY) => {
                Some(StoreFailure::Busy)
            }
            _ => None,
        }
    }

    /// A blocking task that did not come back. Not a full pool, so not
    /// retryable as one.
    pub fn task_failed() -> StoreFailure {
        StoreFailure::Unavailable("the store task did not finish".into())
    }

    /// A blocking task's result, with the task's own failure folded in.
    pub fn joined<T, E: Into<StoreFailure>>(
        r: Result<Result<T, E>, tokio::task::JoinError>,
    ) -> Result<T, StoreFailure> {
        match r {
            Ok(Ok(t)) => Ok(t),
            Ok(Err(e)) => Err(e.into()),
            Err(_) => Err(StoreFailure::task_failed()),
        }
    }

    pub fn is_busy(&self) -> bool {
        matches!(self, StoreFailure::Busy)
    }

    /// The same fact for an op: busy is the ops' own `server_busy`, and
    /// anything else is `space_unavailable` with `otherwise` for its text.
    pub fn exec_error(self, otherwise: &str) -> ordo_api::ExecError {
        match self {
            StoreFailure::Busy => ordo_api::ExecError::server_busy(),
            StoreFailure::Unavailable(_) => {
                ordo_api::ExecError::new(ordo_api::ErrorCode::SpaceUnavailable, otherwise)
            }
        }
    }
}

impl fmt::Display for StoreFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreFailure::Busy => f.write_str(ordo_api::SERVER_BUSY_MESSAGE),
            StoreFailure::Unavailable(why) => f.write_str(why),
        }
    }
}

impl StoreFailure {
    /// The answer, for a route whose broken store has a word of its own
    /// (`link_unavailable`, `tickets_unavailable`, ...). Busy is
    /// `server_busy` whatever the route.
    pub fn answer(self, code: EdgeCode, message: &str) -> Response {
        match self {
            StoreFailure::Busy => ordo_api::ApiError::server_busy().into_response(),
            StoreFailure::Unavailable(_) => EdgeError::new(code, message).into_response(),
        }
    }
}

impl IntoResponse for StoreFailure {
    /// The sign-in routes' answer: busy, or `credential_store_unavailable`.
    fn into_response(self) -> Response {
        self.answer(
            EdgeCode::CredentialStoreUnavailable,
            "credentials cannot be checked right now",
        )
    }
}

/// A pool error as a typed store fault: the class from the pool's own type
/// (`PoolError::class`), the pool error kept as the cause when it is busy
/// (it names no statement), and `words` for anything else. What a store
/// built on a pool fails with; `ExecError::from` and `StoreFailure::from`
/// read its class, never its text.
pub fn pool_fault(e: ordo_exec::PoolError, words: &str) -> ordo_identity::StoreFault {
    if e.is_busy() {
        let text = e.to_string();
        return ordo_identity::StoreFault::caused_by(e.class(), text, e);
    }
    ordo_identity::StoreFault::of_class(e.class(), words)
}

/// A ceremony's error as a store fault, for a store path that calls into
/// one: a store that could not be read keeps its typed fault, a mail outbox
/// refused for a full pool is busy, and anything else is broken, with its
/// sentence.
pub fn auth_fault(e: ordo_identity::AuthError) -> ordo_identity::StoreFault {
    match e {
        ordo_identity::AuthError::StoreUnavailable(c) => c.0,
        ordo_identity::AuthError::UpstreamUnavailable(crate::email_code::MAIL_OUTBOX_BUSY) => {
            ordo_identity::StoreFault::of_class(
                ordo_identity::FaultClass::Busy,
                crate::email_code::MAIL_OUTBOX_BUSY,
            )
        }
        other => ordo_identity::StoreFault::unavailable(other.to_string()),
    }
}

/// An op's error as a store fault, by its code: `server_busy` is busy,
/// anything else is broken, with the op's message.
pub fn exec_fault(e: ordo_api::ExecError) -> ordo_identity::StoreFault {
    let class = if e.code == ordo_api::ErrorCode::ServerBusy {
        ordo_identity::FaultClass::Busy
    } else {
        ordo_identity::FaultClass::Broken
    };
    ordo_identity::StoreFault::of_class(class, e.message)
}

/// Anything a store fails with, as its typed [`ordo_identity::StoreFault`].
///
/// The one conversion every store error goes through on its way to the
/// edge: the class comes from the error's type (`PoolError::class`, a
/// store's own fault, an op's `server_busy` code), never from its words.
/// [`StoreFailure`] and `ordo_api::ExecError` are both built from it.
pub trait IntoFault {
    fn fault(self) -> ordo_identity::StoreFault;
}

impl IntoFault for ordo_identity::StoreFault {
    fn fault(self) -> ordo_identity::StoreFault {
        self
    }
}

/// A sentence with no class of its own: a store that is broken, never busy.
impl IntoFault for String {
    fn fault(self) -> ordo_identity::StoreFault {
        ordo_identity::StoreFault::unavailable(self)
    }
}

impl IntoFault for &str {
    fn fault(self) -> ordo_identity::StoreFault {
        ordo_identity::StoreFault::unavailable(self)
    }
}

/// The pool's class, and its words. The cause is not kept: a statement can
/// carry an identifier ([`pool_fault`] keeps a busy one's).
impl IntoFault for ordo_exec::PoolError {
    fn fault(self) -> ordo_identity::StoreFault {
        ordo_identity::StoreFault::of_class(self.class(), self.to_string())
    }
}

impl IntoFault for ordo_identity::AuthError {
    fn fault(self) -> ordo_identity::StoreFault {
        auth_fault(self)
    }
}

/// An op's error by its code, with its whole text (`code: message`).
impl IntoFault for ordo_api::ExecError {
    fn fault(self) -> ordo_identity::StoreFault {
        let text = self.to_string();
        let class = exec_fault(self).class();
        ordo_identity::StoreFault::of_class(class, text)
    }
}

/// The identity stores' errors carry their fault.
macro_rules! fault_newtype {
    ($($t:ty),* $(,)?) => {
        $(impl IntoFault for $t {
            fn fault(self) -> ordo_identity::StoreFault {
                self.0
            }
        })*
    };
}

fault_newtype!(
    ordo_identity::CredentialStoreError,
    ordo_identity::ceremony::CeremonyStoreError,
    ordo_identity::directory::DirectoryStoreError,
    ordo_identity::session::RevocationStoreError,
    crate::email_code::EmailStoreError,
    crate::federation::IdentityAttrError,
);

impl IntoFault for ordo_identity::DirectoryError {
    fn fault(self) -> ordo_identity::StoreFault {
        match self {
            ordo_identity::DirectoryError::StoreUnavailable(s) => s.0,
            other => ordo_identity::StoreFault::unavailable(other.to_string()),
        }
    }
}

impl IntoFault for ordo_identity::SessionError {
    fn fault(self) -> ordo_identity::StoreFault {
        match self {
            ordo_identity::SessionError::StoreUnavailable(s) => s.0,
            other => ordo_identity::StoreFault::unavailable(other.to_string()),
        }
    }
}

impl IntoFault for ordo_identity::passkey::PasskeyStoreError {
    fn fault(self) -> ordo_identity::StoreFault {
        let text = self.to_string();
        match self {
            ordo_identity::passkey::PasskeyStoreError::Unavailable(f) => {
                ordo_identity::StoreFault::of_class(f.class(), text)
            }
            _ => ordo_identity::StoreFault::unavailable(text),
        }
    }
}

impl IntoFault for ordo_identity::grants::GrantError {
    fn fault(self) -> ordo_identity::StoreFault {
        let text = self.to_string();
        match self {
            ordo_identity::grants::GrantError::Unavailable(f) => {
                ordo_identity::StoreFault::of_class(f.class(), text)
            }
            _ => ordo_identity::StoreFault::unavailable(text),
        }
    }
}

impl IntoFault for ordo_identity::delegation::DelegationError {
    fn fault(self) -> ordo_identity::StoreFault {
        let text = self.to_string();
        match self {
            ordo_identity::delegation::DelegationError::Unavailable(f) => {
                ordo_identity::StoreFault::of_class(f.class(), text)
            }
            _ => ordo_identity::StoreFault::unavailable(text),
        }
    }
}

impl IntoFault for crate::mail::MailError {
    fn fault(self) -> ordo_identity::StoreFault {
        let text = self.to_string();
        ordo_identity::StoreFault::of_class(self.class(), text)
    }
}

impl IntoFault for crate::tickets::TicketError {
    fn fault(self) -> ordo_identity::StoreFault {
        let text = self.to_string();
        match self {
            crate::tickets::TicketError::Unavailable(f) => {
                ordo_identity::StoreFault::of_class(f.class(), text)
            }
            _ => ordo_identity::StoreFault::unavailable(text),
        }
    }
}

/// Every store error converts by its typed class ([`IntoFault`]).
impl<E: IntoFault> From<E> for StoreFailure {
    fn from(e: E) -> StoreFailure {
        let f = e.fault();
        let text = f.to_string();
        StoreFailure::from_fault(&f, text)
    }
}

/// A store error as an op's error: busy is `server_busy`, anything else
/// `space_unavailable` with the store's sentence (`ExecError::from`).
pub fn exec_unavailable(e: impl IntoFault) -> ordo_api::ExecError {
    ordo_api::ExecError::from(e.fault())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn a_full_pool_is_busy_by_its_type_through_any_wrapper() {
        let busy = || crate::passkeys::scrub_fault("$principal", ordo_exec::PoolError::Saturated);
        assert!(busy().is_busy());
        assert!(
            std::error::Error::source(&busy()).is_some(),
            "the pool error is the cause"
        );
        let wrapped = ordo_identity::DirectoryError::StoreUnavailable(
            ordo_identity::directory::DirectoryStoreError(busy()),
        );
        assert!(StoreFailure::from(wrapped).is_busy());
        let auth =
            ordo_identity::AuthError::StoreUnavailable(ordo_identity::CredentialStoreError(busy()));
        assert_eq!(StoreFailure::from_auth(&auth), Some(StoreFailure::Busy));
        // The class, not the words: a store sentence that happens to quote
        // the pool's message is not busy unless its type says so.
        let words =
            ordo_identity::CredentialStoreError(ordo_exec::PoolError::Saturated.to_string().into());
        assert!(!StoreFailure::from(words).is_busy());
        assert!(StoreFailure::from(ordo_exec::PoolError::Saturated).is_busy());
    }

    #[test]
    fn the_stores_typed_in_step_two_keep_busy_by_their_type() {
        let busy = pool_fault(ordo_exec::PoolError::Saturated, "the ticket table");
        assert!(busy.is_busy());
        let ticket = crate::tickets::TicketError::Unavailable(busy.clone());
        assert!(StoreFailure::from(ticket).is_busy());
        let grant = ordo_identity::grants::GrantError::Unavailable(busy.clone());
        assert!(StoreFailure::from(grant).is_busy());
        let op = ordo_api::ExecError::from(busy.clone());
        assert_eq!(op.code, ordo_api::ErrorCode::ServerBusy);
        assert!(StoreFailure::from(op).is_busy());
        // A broken store stays broken, whatever it is wrapped in.
        let broken = pool_fault(ordo_exec::PoolError::db("gone"), "the ticket table");
        assert!(!StoreFailure::from(crate::tickets::TicketError::Unavailable(broken)).is_busy());
        // The words alone are not busy: only the type says so.
        let words = ordo_exec::PoolError::Saturated.to_string();
        assert!(!StoreFailure::from(words).is_busy());
    }

    #[test]
    fn anything_else_is_unavailable() {
        let f = StoreFailure::from("`$principal` does not exist");
        assert!(!f.is_busy());
        let res = f.into_response();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(res.headers().get("retry-after").is_none());
        assert_eq!(
            StoreFailure::from_auth(&ordo_identity::AuthError::BadCredentials),
            None
        );
    }

    #[test]
    fn busy_answers_server_busy_with_retry_after() {
        let res = StoreFailure::Busy.into_response();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(res.headers().get("retry-after").unwrap(), "1");
    }
}
