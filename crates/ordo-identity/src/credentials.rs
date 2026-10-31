//! Where a password credential is kept, behind a trait.
//!
//! [`crate::provider::LocalPasswordProvider`] used to own a `BTreeMap` of
//! identifier → hash. That map was the whole credential table of a
//! deployment, it lived in one process's heap, and it was empty again the
//! moment that process restarted. Everything above it worked: the argon2id
//! profile, the anti-enumeration refusal, the ceremony expiry, and none of
//! it survived a deploy. This trait is the seam that makes the storage
//! choice separate from the verification, the same shape
//! `ordo_crypto::WrappedKeyStore` gives custody.
//!
//! # What may be stored
//!
//! A **PHC string** and nothing else. `hash` is what
//! [`crate::provider::PasswordHasher::hash`] returned, which carries its own
//! algorithm and cost parameters and from which the password cannot be read.
//! No implementation of this trait is ever handed a password, so no
//! implementation can store one, log one, or put one in an error: the
//! plaintext never leaves the provider's stack frame.
//!
//! # Why the trait is synchronous
//!
//! [`crate::provider::AuthProvider::complete`] is synchronous, because the
//! provider surface predates any HTTP in this tree and because a ceremony is
//! a pure function of its inputs. A store behind it is therefore synchronous
//! too, and an implementation over an async pool owns the bridge, see
//! `ordo_host::credentials::PgCredentialStore`, which follows
//! `ordo_exec::keystore::PgKeyStore`'s thread-with-its-own-runtime pattern
//! for exactly this reason.

use std::collections::BTreeMap;
use std::sync::RwLock;

/// The store could not answer. **Distinct from a wrong password on
/// purpose**, and it discloses nothing: whether the database is reachable
/// does not depend on which identifier was offered, so an attacker learns
/// the same fact from any request and learns nothing about any account.
///
/// Conflating the two would be the harmful direction. A deployment whose
/// credential table is unreachable would tell every caller "wrong password",
/// and the first thing a user does about that is change a password they
/// remembered correctly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialStoreError(pub crate::StoreFault);

impl std::fmt::Display for CredentialStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "credential store unavailable: {}", self.0)
    }
}

impl From<crate::CeremonyStoreError> for CredentialStoreError {
    /// A ceremony store that failed under a credential check: its class and
    /// its words are kept, and it is the cause.
    fn from(e: crate::CeremonyStoreError) -> CredentialStoreError {
        CredentialStoreError(crate::StoreFault::caused_by(e.class(), e.to_string(), e))
    }
}

impl From<crate::passkey::PasskeyStoreError> for CredentialStoreError {
    fn from(e: crate::passkey::PasskeyStoreError) -> CredentialStoreError {
        let class = match &e {
            crate::passkey::PasskeyStoreError::Unavailable(f) => f.class(),
            crate::passkey::PasskeyStoreError::AlreadyExists => crate::FaultClass::Broken,
        };
        CredentialStoreError(crate::StoreFault::caused_by(class, e.to_string(), e))
    }
}

impl CredentialStoreError {
    /// Busy or unavailable, as the store's client said.
    pub fn class(&self) -> crate::FaultClass {
        self.0.class()
    }
}

impl std::error::Error for CredentialStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// Identifier → stored hash.
///
/// `Send + Sync` and interior-mutable: a provider is shared across requests
/// for the life of the process (see [`crate::provider::AuthProvider`]), so a
/// store it holds is written through `&self` or it could not be written at
/// all.
pub trait CredentialStore: Send + Sync {
    /// The stored PHC string for `identifier`, or `None` if nothing is
    /// enrolled under it.
    ///
    /// **`None` and an error are different answers and the caller keeps them
    /// apart.** `None` is "no such credential", which the provider turns
    /// into the same refusal as a wrong password; an error is "ask again
    /// later".
    fn hash_for(&self, identifier: &str) -> Result<Option<String>, CredentialStoreError>;

    /// Enroll or re-enroll `identifier` under `hash`.
    ///
    /// Replacing rather than refusing is right here and is the opposite of
    /// [`crate::provider::AuthError::CredentialAlreadyEnrolled`]'s rule for
    /// passkeys, because the two acts differ: replacing a password is what a
    /// password change *is*, and it is authorized by whoever called this,
    /// while replacing a passkey's public key under an existing
    /// authenticator id hands the account to whoever asked. This trait is
    /// reached only from an enrolment path that has already decided the
    /// caller may do it.
    fn put(&self, identifier: &str, hash: &str) -> Result<(), CredentialStoreError>;

    /// Remove any credential under `identifier`. Removing one that is not
    /// there is success: the postcondition is "nothing is enrolled", and a
    /// caller that retries a delete must not get an error the second time.
    fn remove(&self, identifier: &str) -> Result<(), CredentialStoreError>;
}

/// The process-local store.
///
/// **For tests, and for nothing a deployment runs.** It is the behaviour
/// `LocalPasswordProvider` used to have inline, kept because a test about a
/// password should not need a database, and kept *named*, so that a
/// deployment wired to it is visible in a diff rather than implied by the
/// absence of configuration. `ordo_host::runtime` refuses to boot with a
/// password method and no durable store rather than falling back to this.
#[derive(Debug, Default)]
pub struct MemoryCredentialStore {
    rows: RwLock<BTreeMap<String, String>>,
}

impl MemoryCredentialStore {
    pub fn new() -> MemoryCredentialStore {
        MemoryCredentialStore::default()
    }

    /// How many credentials are enrolled. Test observation only; the trait
    /// deliberately has no listing, because a credential table that can be
    /// enumerated through the same interface a login uses is one mistake
    /// away from being enumerated by a login.
    pub fn len(&self) -> usize {
        self.rows.read().expect("credential store").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn hash_for(&self, identifier: &str) -> Result<Option<String>, CredentialStoreError> {
        Ok(self
            .rows
            .read()
            .expect("credential store")
            .get(identifier)
            .cloned())
    }

    fn put(&self, identifier: &str, hash: &str) -> Result<(), CredentialStoreError> {
        self.rows
            .write()
            .expect("credential store")
            .insert(identifier.to_string(), hash.to_string());
        Ok(())
    }

    fn remove(&self, identifier: &str) -> Result<(), CredentialStoreError> {
        self.rows
            .write()
            .expect("credential store")
            .remove(identifier);
        Ok(())
    }
}
