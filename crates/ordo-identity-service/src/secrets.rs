//! Ids, secrets and how a secret is stored.
//!
//! - An **id** (`p_…` a person, `s_…` a session, `m_…` a machine key,
//!   `i_…` an invite) is 128 random bits, hex. It is public.
//! - A **session secret** is 256 random bits, base64url. Only its tagged
//!   SHA-256 is stored, and the hash is what finds the row.
//! - An **invite secret** and a **CLI code** are codes
//!   (`ordo_identity::codes`, 120 bits); only their tagged SHA-256 is
//!   stored. A **recovery code** is the console's, unchanged, so the codes
//!   the console printed keep working here.

pub use ordo_identity::codes::{random_bytes, tagged_hash};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh id: `prefix` and 32 hex characters.
pub fn mint_id(prefix: &str) -> String {
    format!("{prefix}{}", hex(&random_bytes::<16>()))
}

/// A fresh session secret, base64url.
pub fn mint_secret() -> String {
    ordo_identity::webauthn::b64url_encode(&random_bytes::<32>())
}

/// What a session secret is stored as.
pub fn session_hash(secret: &str) -> String {
    tagged_hash("ordo-identity/session/v1", secret)
}

/// What an invite secret is stored as (normalised first).
pub fn invite_hash(normalised: &str) -> String {
    tagged_hash("ordo-identity/invite/v1", normalised)
}

/// What a CLI grant's code is stored as.
pub fn cli_code_hash(code: &str) -> String {
    tagged_hash("ordo-identity/cli-code/v1", code)
}
