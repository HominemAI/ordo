//! Issuing deployment tokens (`plan-identity-service.md` 4.1).
//!
//! The claim set and the signature are `ordo_identity::deployment_token`'s,
//! the same module the door and the console verify with. [`issue`] is the
//! one place this service fills a claim set: a session's token, a machine
//! key's token and a break-glass token all come through it, so a change to
//! the claims is a change here and in `DeploymentClaims`, nowhere else.
//!
//! Tokens are not audit rows; the sessions they come from are, and so is
//! every break-glass token.

use ordo_identity::deployment_token::{self, DeploymentClaims, Kind, ROLE_ADMIN};
use ordo_identity::webauthn::b64url_encode;

use crate::error::IdentityError;
use crate::keys::KeyRing;
use crate::secrets::random_bytes;
use crate::sessions::{Live, SessionKind};

/// A signed token and what it says.
#[derive(Debug, Clone)]
pub struct Issued {
    pub token: String,
    pub claims: DeploymentClaims,
    pub kid: String,
}

/// What the issuer stamps on every token.
#[derive(Debug, Clone, Copy)]
pub struct Issuer<'a> {
    /// `ORDO_IDENTITY_ORIGIN`: the `iss`.
    pub origin: &'a str,
    /// `ordo-deployment:<handle>`: the `aud`.
    pub audience: &'a str,
    /// `ORDO_IDENTITY_TOKEN_SECS`.
    pub token_secs: u64,
}

/// Who a token is for.
#[derive(Debug, Clone, Copy)]
pub struct Subject<'a> {
    pub sub: &'a str,
    pub kind: Kind,
    pub client: &'a str,
    pub sid: &'a str,
    pub touched_at: Option<u64>,
}

/// Sign a token for `who`, living `life_secs`. The one place a claim set is
/// filled.
pub fn issue(
    issuer: Issuer<'_>,
    keys: &KeyRing,
    who: Subject<'_>,
    now: u64,
    life_secs: u64,
) -> Result<Issued, IdentityError> {
    let claims = DeploymentClaims {
        iss: issuer.origin.to_string(),
        aud: issuer.audience.to_string(),
        sub: who.sub.to_string(),
        kind: who.kind,
        role: ROLE_ADMIN.to_string(),
        client: who.client.to_string(),
        sid: who.sid.to_string(),
        // A machine token never carries a touch, whatever it was handed.
        touched_at: match who.kind {
            Kind::Machine => None,
            _ => who.touched_at,
        },
        iat: now,
        exp: now + life_secs,
        jti: b64url_encode(&random_bytes::<16>()),
    };
    let (kid, key) = keys.token_signer(now)?;
    Ok(Issued {
        token: deployment_token::sign(&claims, &kid, &key),
        claims,
        kid,
    })
}

/// The token for a live session, for the token life.
pub fn for_session(
    issuer: Issuer<'_>,
    keys: &KeyRing,
    session: &Live,
    now: u64,
) -> Result<Issued, IdentityError> {
    let kind = match session.kind {
        SessionKind::Browser | SessionKind::Cli => Kind::Person,
        SessionKind::Machine => Kind::Machine,
    };
    issue(
        issuer,
        keys,
        Subject {
            sub: &session.subject,
            kind,
            client: &session.client,
            sid: &session.id,
            touched_at: session.touched_at,
        },
        now,
        issuer.token_secs,
    )
}
