//! What kind of failure a service had, typed once and shared by every layer
//! above it (AGENTS.md rule 9, owner 2026-10-23).
//!
//! The database client (`ordo_exec::PoolError::class`) and every store built
//! on it (`ordo_identity`'s store errors, through [`StoreFault`]) answer the
//! same question with the same type: what can a caller do about this? The
//! answer is read from the error's type, never from its words. One place at
//! the server's edge turns it into an HTTP answer.

mod store_fault;

pub use store_fault::StoreFault;

/// What a caller can do about a failure.
///
/// `#[non_exhaustive]`: a class added later must be decided by every
/// caller outside this crate, through its wildcard arm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FaultClass {
    /// Every connection the deployment holds was in use. Nothing ran, and
    /// the same request can be sent again shortly. Answered `server_busy`.
    Busy,
    /// This attempt did not happen and nothing it did landed: a
    /// serialization conflict, a deadlock, a lock wait that ran out, a
    /// retry budget spent. Sending the same work again later is correct.
    Retryable,
    /// The request, not the server: a declared check refused the data, the
    /// arithmetic failed, a statement ran past the deployment's time bound,
    /// a constraint or a value was refused. Sending it again unchanged
    /// gets the same answer; the caller fixes the input.
    Refused,
    /// A stored value failed to authenticate or decrypt while its key was
    /// present: unreadable, possibly tampered (owner, 2026-10-23). Never
    /// retryable, and never "destroyed", which means only a key someone
    /// erased on purpose. The deployment owner is alerted.
    Unreadable,
    /// Anything else: a missing table, a dead database, a protocol fault.
    #[default]
    Broken,
}

impl FaultClass {
    pub fn is_busy(self) -> bool {
        self == FaultClass::Busy
    }

    /// Busy or retryable: the same work, sent again later, may land.
    pub fn may_retry(self) -> bool {
        matches!(self, FaultClass::Busy | FaultClass::Retryable)
    }

    /// The class in one word, for a log line or a metric label.
    pub fn as_str(self) -> &'static str {
        match self {
            FaultClass::Busy => "busy",
            FaultClass::Retryable => "retryable",
            FaultClass::Refused => "refused",
            FaultClass::Unreadable => "unreadable",
            FaultClass::Broken => "broken",
        }
    }
}

impl std::fmt::Display for FaultClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_busy_and_retryable_may_retry() {
        assert!(FaultClass::Busy.may_retry());
        assert!(FaultClass::Retryable.may_retry());
        assert!(!FaultClass::Refused.may_retry());
        assert!(!FaultClass::Unreadable.may_retry());
        assert!(!FaultClass::Broken.may_retry());
        assert_eq!(FaultClass::default(), FaultClass::Broken);
    }

    #[test]
    fn a_fault_keeps_its_class_and_its_cause() {
        let cause = std::io::Error::other("socket closed");
        let f = StoreFault::caused_by(FaultClass::Busy, "busy", cause);
        assert!(f.is_busy());
        let source = std::error::Error::source(&f).expect("the cause is kept");
        assert_eq!(source.to_string(), "socket closed");
        assert_eq!(StoreFault::unavailable("gone").class(), FaultClass::Broken);
    }
}
