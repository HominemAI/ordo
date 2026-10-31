//! Where an open ceremony is kept, behind a trait.
//!
//! [`crate::passkey::PasskeyProvider`] used to hold its open challenges in a
//! `BTreeMap` on the provider. A challenge minted by one replica could not be
//! completed by another, and a restart forgot every ceremony in flight. This
//! trait is the seam that moves them into the control database
//! (`plan-passkey-signin.md` §2.4), in the shape `plan-auth-federated.md` §3.1
//! defined so that outside-provider sign-in extends the same table.
//!
//! # What is stored, and what is not
//!
//! The row's **handle** is the SHA-256 of the challenge, never the challenge.
//! A caller completing a ceremony presents the challenge, so the server can
//! recompute the handle; a reader of the table cannot go the other way. The
//! `binder` column is empty for a passkey, because a WebAuthn challenge is
//! bound by the signature over `clientDataJSON`; it is there for a provider
//! whose ceremony needs to be bound to a browser some other way.
//!
//! # One-shot means the database decides
//!
//! [`CeremonyStore::take`] is a delete that returns what it deleted. Two
//! callers racing on one handle cannot both get it, on one replica or on two,
//! because the row can only be deleted once.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::principal::Timestamp;

/// One open ceremony.
///
/// Phases: `register`, `assert` and `step-up` for a passkey; `begun` and
/// `handoff` for an outside provider (`plan-auth-federated.md` §3.1). A
/// `handoff` row is written by the provider's callback and lives sixty
/// seconds; its payload is the subject the provider vouched for, and it is
/// redeemed only with the binder its `begun` row was filed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CeremonyRow {
    /// SHA-256 of the challenge, base64url. The key.
    pub handle: String,
    /// `passkey` here; a connection name for an outside provider.
    pub connection: String,
    /// `register`, `assert` or `step-up` here.
    pub phase: String,
    /// Empty for a passkey. See the module doc.
    pub binder: String,
    /// JSON the provider wants back at completion: a user handle, a ticket
    /// reference, the principal a step-up is for.
    pub payload: String,
    pub minted_at: Timestamp,
    /// The instant after which the row answers nothing. `take` at exactly
    /// this instant finds nothing.
    pub expires_at: Timestamp,
}

/// The store could not answer. Distinct from "there is no such ceremony",
/// for the reason [`crate::credentials::CredentialStoreError`] is distinct
/// from a wrong password: whether the database is reachable does not depend
/// on which ceremony was offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CeremonyStoreError(pub crate::StoreFault);

impl std::fmt::Display for CeremonyStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ceremony store unavailable: {}", self.0)
    }
}

impl CeremonyStoreError {
    /// Busy or unavailable, as the store's client said.
    pub fn class(&self) -> crate::FaultClass {
        self.0.class()
    }
}

impl std::error::Error for CeremonyStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

pub trait CeremonyStore: Send + Sync {
    /// Store an open ceremony.
    fn put(&self, row: &CeremonyRow) -> Result<(), CeremonyStoreError>;

    /// Delete the row for `handle` **that matches `connection` and `phase`
    /// and has not expired at `now`**, and return its payload. Absent,
    /// expired and of another phase are the same `None`. The store decides
    /// which of two racing callers wins.
    ///
    /// A row of another phase is left where it is: a step-up challenge
    /// presented at the sign-in route does not consume the step-up.
    fn take(
        &self,
        handle: &str,
        connection: &str,
        phase: &str,
        now: Timestamp,
    ) -> Result<Option<String>, CeremonyStoreError>;

    /// [`CeremonyStore::take`], and the row's `binder` must also equal
    /// `binder`. A row whose binder differs is **left where it is** and the
    /// answer is `None`, so a handoff planted in another browser cannot be
    /// burned by that browser, and is refused to it.
    ///
    /// `binder` is the stored form, the SHA-256 of what the page holds,
    /// computed by the caller. The comparison happens in the store's own
    /// predicate, so it is one statement and one winner as `take` is.
    fn take_bound(
        &self,
        handle: &str,
        connection: &str,
        phase: &str,
        binder: &str,
        now: Timestamp,
    ) -> Result<Option<String>, CeremonyStoreError>;

    /// Read the payload of the row `take` would return, **without**
    /// deleting it. For a caller that must know a ceremony is open before
    /// it does something that is not the ceremony's completion: an emailed
    /// code is mailed to an unknown address when the `begin` names an open
    /// claim (`plan-auth-federated.md` step 7), and the claim is completed
    /// later with `take`.
    fn peek(
        &self,
        handle: &str,
        connection: &str,
        phase: &str,
        now: Timestamp,
    ) -> Result<Option<String>, CeremonyStoreError>;

    /// Delete every row that has expired at `now`. Returns how many.
    fn sweep(&self, now: Timestamp) -> Result<u64, CeremonyStoreError>;
}

/// The process-local store. **For tests, and for nothing a deployment
/// runs.** It is what the provider held inline before the trait existed.
#[derive(Debug, Default)]
pub struct MemoryCeremonyStore {
    rows: Mutex<BTreeMap<String, CeremonyRow>>,
}

impl MemoryCeremonyStore {
    pub fn new() -> MemoryCeremonyStore {
        MemoryCeremonyStore::default()
    }

    /// How many ceremonies are open. Test observation only.
    pub fn len(&self) -> usize {
        self.rows.lock().expect("ceremony store").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl CeremonyStore for MemoryCeremonyStore {
    fn put(&self, row: &CeremonyRow) -> Result<(), CeremonyStoreError> {
        self.rows
            .lock()
            .expect("ceremony store")
            .insert(row.handle.clone(), row.clone());
        Ok(())
    }

    fn take(
        &self,
        handle: &str,
        connection: &str,
        phase: &str,
        now: Timestamp,
    ) -> Result<Option<String>, CeremonyStoreError> {
        let mut rows = self.rows.lock().expect("ceremony store");
        let matches = rows.get(handle).is_some_and(|r| {
            r.connection == connection && r.phase == phase && r.expires_at.0 > now.0
        });
        if !matches {
            return Ok(None);
        }
        Ok(rows.remove(handle).map(|r| r.payload))
    }

    fn take_bound(
        &self,
        handle: &str,
        connection: &str,
        phase: &str,
        binder: &str,
        now: Timestamp,
    ) -> Result<Option<String>, CeremonyStoreError> {
        let mut rows = self.rows.lock().expect("ceremony store");
        let matches = rows.get(handle).is_some_and(|r| {
            r.connection == connection
                && r.phase == phase
                && r.binder == binder
                && r.expires_at.0 > now.0
        });
        if !matches {
            return Ok(None);
        }
        Ok(rows.remove(handle).map(|r| r.payload))
    }

    fn peek(
        &self,
        handle: &str,
        connection: &str,
        phase: &str,
        now: Timestamp,
    ) -> Result<Option<String>, CeremonyStoreError> {
        let rows = self.rows.lock().expect("ceremony store");
        Ok(rows
            .get(handle)
            .filter(|r| r.connection == connection && r.phase == phase && r.expires_at.0 > now.0)
            .map(|r| r.payload.clone()))
    }

    fn sweep(&self, now: Timestamp) -> Result<u64, CeremonyStoreError> {
        let mut rows = self.rows.lock().expect("ceremony store");
        let before = rows.len();
        rows.retain(|_, r| r.expires_at.0 > now.0);
        Ok((before - rows.len()) as u64)
    }
}
