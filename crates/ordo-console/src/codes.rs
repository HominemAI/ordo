//! Codes, tokens and the clock.
//!
//! The code shapes are `ordo_identity::codes`, shared with the identity
//! service so a recovery code the console printed keeps working there
//! (`plan-identity-service.md` 2.1). What stays here is the console's own:
//!
//! - A **setup code** (first run, a second owner's invite, `ordo console
//!   recover`) is a code whose SHA-256 under the console's tag is stored.
//! - A **session token** is 256 random bits; only its tagged SHA-256 is
//!   stored.

pub use ordo_identity::codes::{
    display_code, mint_code, normalise_code, random_bytes, tagged_hash, CODE_LEN, LOOKUP_LEN,
    RECOVERY_SET_SIZE,
};

/// What a setup code is stored as.
pub fn setup_code_hash(normalised: &str) -> String {
    tagged_hash("ordo-console/setup-code/v1", normalised)
}

/// What a session token is stored as.
pub fn session_hash(token: &str) -> String {
    tagged_hash("ordo-console/session/v1", token)
}

/// A fresh session token, base64url.
pub fn mint_session_token() -> String {
    ordo_identity::webauthn::b64url_encode(&random_bytes::<32>())
}

/// A fresh owner id, `own_` and 32 hex characters.
pub fn mint_owner_id() -> String {
    let hex: String = random_bytes::<16>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("own_{hex}")
}

/// The clock, shared with the identity service.
pub use ordo_identity::clock::Clock;
