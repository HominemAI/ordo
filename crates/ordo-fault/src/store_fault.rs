//! Why a store did not answer, typed: the one fault every store error
//! carries (`ordo_identity`'s `CredentialStoreError`, `CeremonyStoreError`,
//! `DirectoryStoreError`, `RevocationStoreError`,
//! `PasskeyStoreError::Unavailable`).
//!
//! The class is read once, from the database client's own typed error
//! (`ordo_exec::PoolError::class`), where the store is built, and carried
//! up by value. Nothing above decides by reading the text (owner rulings
//! 2026-10-23: busy versus broken through one interface, and one typed
//! error per service).

use std::fmt;
use std::sync::Arc;

use crate::FaultClass;

/// A store failure: a sentence for the owner's log, its [`FaultClass`], and
/// the client error underneath when it is safe to keep (a statement can
/// carry an identifier, so a store keeps the cause only when it names none).
#[derive(Clone)]
pub struct StoreFault {
    reason: String,
    class: FaultClass,
    source: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl StoreFault {
    /// An unavailable store, with the sentence the log reads.
    pub fn unavailable(reason: impl Into<String>) -> StoreFault {
        StoreFault {
            reason: reason.into(),
            class: FaultClass::Broken,
            source: None,
        }
    }

    /// A fault of a given class, with the client error that caused it.
    pub fn caused_by(
        class: FaultClass,
        reason: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> StoreFault {
        StoreFault {
            reason: reason.into(),
            class,
            source: Some(Arc::new(source)),
        }
    }

    /// A fault of a given class with no cause kept.
    pub fn of_class(class: FaultClass, reason: impl Into<String>) -> StoreFault {
        StoreFault {
            reason: reason.into(),
            class,
            source: None,
        }
    }

    pub fn class(&self) -> FaultClass {
        self.class
    }

    pub fn is_busy(&self) -> bool {
        self.class == FaultClass::Busy
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl From<String> for StoreFault {
    fn from(reason: String) -> StoreFault {
        StoreFault::unavailable(reason)
    }
}

impl From<&str> for StoreFault {
    fn from(reason: &str) -> StoreFault {
        StoreFault::unavailable(reason)
    }
}

impl PartialEq for StoreFault {
    fn eq(&self, other: &StoreFault) -> bool {
        self.reason == other.reason && self.class == other.class
    }
}

impl Eq for StoreFault {}

impl fmt::Debug for StoreFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreFault")
            .field("reason", &self.reason)
            .field("class", &self.class)
            .finish()
    }
}

impl fmt::Display for StoreFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for StoreFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn std::error::Error + 'static))
    }
}
