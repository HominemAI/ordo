//! Seconds since the epoch, from the system or from a test.
//!
//! Shared by the console and the identity service, so a test of either can
//! stand the clock still and move it by hand (session idle and absolute
//! ends, token expiry, key rotation).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Seconds since the epoch, from the system or from a test.
#[derive(Clone, Debug, Default)]
pub struct Clock(Option<Arc<AtomicU64>>);

impl Clock {
    /// The system clock.
    pub fn system() -> Clock {
        Clock(None)
    }

    /// A clock that stands still until a test moves it.
    pub fn manual(start: u64) -> Clock {
        Clock(Some(Arc::new(AtomicU64::new(start))))
    }

    /// Move a manual clock forward. Does nothing to the system clock.
    pub fn advance(&self, secs: u64) {
        if let Some(t) = &self.0 {
            t.fetch_add(secs, Ordering::SeqCst);
        }
    }

    pub fn now(&self) -> u64 {
        match &self.0 {
            Some(t) => t.load(Ordering::SeqCst),
            None => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }
}
