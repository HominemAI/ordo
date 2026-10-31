//! Identity & access for the Ordo platform: principal directory, pluggable
//! authentication providers, account linking, and the one-check-at-the-door
//! access engine (spec §8).
//!
//! Design companion: `docs/security-logging-design.md`. The ruling implemented
//! here: in-house core (directory, grants, step-up) with **every**
//! authentication method behind the [`provider::AuthProvider`] trait —
//! Keycloak, Google, Discord, enterprise SAML are configurations of a
//! provider, never architectural forks.
//!
//! # Where presence comes from
//!
//! The one thing in here that is not method-blind is **user presence**. The
//! design makes passkeys "the step-up source — user-presence assertions come
//! only from here" (§2) and makes presence non-delegable (§5), so
//! [`passkey::UserPresence`] is constructible in exactly one module and
//! travels one path: [`passkey::PasskeyProvider::complete`] →
//! [`provider::AuthenticatedSubject::presence`] → [`session::sign_in`] →
//! [`token::TokenSpec::user_presence_at`] → [`access::TokenClaims`] →
//! [`access::StepUpRequirement`]. Every link in that chain is a type, not a
//! convention.
//!
//! **One second path, off by default** (`plan-auth-federated.md` §8.2, owner
//! 2026-09-29): an OIDC connection's fresh two-step claim, where the app
//! developer's setting lets it count. It is its own type,
//! [`upstream::UpstreamPresence`], carried in the session claim and never in
//! `user_presence_at`, and only [`access::AccessEngine`] reads it, through
//! [`upstream::UpstreamAcceptance`]. Everything that reads a passkey's
//! presence directly still needs a passkey.

pub mod access;
pub mod ceremony;
pub mod clock;
pub mod codes;
pub mod credentials;
pub mod delegation;
pub mod deployment_token;
pub mod directory;
pub mod federation;
pub mod grants;
pub mod jose;
pub mod oauth2;
pub mod oidc;
pub mod origin;
pub mod passkey;
pub mod pins;
pub mod principal;
pub mod provider;
pub mod registry;
pub mod sealing;
pub mod second_step;
pub mod session;
/// A software authenticator for tests. Behind `test-support`; a deployment
/// never compiles it.
#[cfg(any(test, feature = "test-support"))]
pub mod software_authenticator;
/// The store fault and its class live in `ordo-fault`, where the database
/// client shares them.
pub use ordo_fault as store_fault;
pub mod token;
pub mod totp;
pub mod upstream;
pub mod webauthn;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod passkey_tests;

pub use access::{
    AccessEngine, Decision, DenyReason, StepUpRequirement, TokenClaims,
    DEFAULT_STEP_UP_MAX_AGE_SECS,
};
pub use ceremony::{CeremonyRow, CeremonyStore, CeremonyStoreError, MemoryCeremonyStore};
pub use credentials::{CredentialStore, CredentialStoreError, MemoryCredentialStore};
pub use delegation::{
    DelegationClaim, DelegationError, DelegationRecord, DelegationRegistry, NewDelegation,
};
pub use directory::{only_outside_provider, OUTSIDE_PROVIDER};
pub use directory::{
    AuditEvent, AuditEventKind, Contested, Directory, DirectoryError, DirectoryStore,
    DirectoryStoreError, LinkCeremony, LinkMeta, MemoryDirectoryStore, Onboarding, Provisioning,
    Recovered, RecoveryTicket, Retired,
};
pub use federation::{
    set_handoff_ttl_secs, ClientSecret, FederationMethod, FederationRequest, FederationResponse,
    FederationTransport, ProviderError, HANDOFF_TTL_SECS,
};
pub use ordo_fault::{FaultClass, StoreFault};
pub use passkey::{
    CreationOptions, KeyRole, MemoryPasskeyStore, PasskeyCredential, PasskeyProvider, PasskeyStore,
    PasskeyStoreError, PendingRegistration, RelyingParty, RelyingPartyError, RequestOptions,
    SecondStepProof, StoredPasskey, UserPresence, UserVerification, CHALLENGE_TTL_SECS,
};
pub use pins::{compare, decode_pins, encode_pins, EntityPin, GuardPin, OpPin, PinDrift};
pub use principal::{
    AccessProfile, AuthMethodKind, CapabilityGrant, ClientId, DelegationId, GrantId, GroupId,
    ManifestHash, MemberOverride, MembershipStatus, Principal, PrincipalId, PrincipalKind, Role,
    SpaceId, Timestamp,
};
pub use provider::{
    Argon2idHasher, AuthError, AuthProvider, AuthenticatedSubject, AuthenticatorId, BeginRequest,
    ChallengeId, ChallengeOrRedirect, CompleteRequest, EmailEvidence, LocalPasswordProvider,
    PasswordHasher, SubjectAttributes, UpstreamAssurance, VerifiedEmail, ARGON2ID_M_COST_KIB,
    ARGON2ID_OUTPUT_LEN, ARGON2ID_P_COST, ARGON2ID_T_COST, CEREMONY_TTL_SECS,
};
pub use registry::{ProviderRegistry, RegistryError};
pub use second_step::{DeploymentMode, PersonMode, SecondStepKind, TenantMode};
pub use session::{
    logout, refresh, refresh_presence, sign_in, sign_in_as, LifetimeError,
    MemorySessionRevocations, RevocationStoreError, SessionClaim, SessionError, SessionId,
    SessionRevocations, SignIn, DEFAULT_SESSION_MAX_SECS, DEFAULT_SESSION_SECS,
};
pub use token::{mint, verify, TokenError, TokenKey, TokenKeyError, TokenSpec};
pub use upstream::{PresenceMode, PresenceRule, UpstreamAcceptance, UpstreamPresence};
pub use webauthn::WebAuthnError;
