//! The identity service's side of the matrix (`plan-identity-service.md`
//! 4.3, 8): a fake issuer, a signer and a key set on a real loopback
//! socket, and the deployment tokens it mints.
//!
//! The host binary is told where the keys are (`ORDO_IDENTITY_URL`), which
//! `iss` and which `aud` to expect. Every token here is a real 3 segment
//! JWS the door verifies against the published set. The tokens that must
//! fail each break one thing and keep the rest right.

use std::io::{Read, Write};
use std::net::TcpListener;

use ed25519_dalek::SigningKey;
use ordo_identity::deployment_token::{self, DeploymentClaims, Kind, ROLE_ADMIN};
use serde_json::json;

use super::actors::now_secs;

/// The `iss` the host is told (`ORDO_IDENTITY_ORIGIN`).
pub const ISS: &str = "https://id.matrix.example";
/// The handle the host is told (`ORDO_IDENTITY_DEPLOYMENT`).
pub const HANDLE: &str = "matrix";

/// How long ago a stale touch was: well past every touch window.
const STALE_SECS: u64 = 7_200;

/// The fake identity service: it publishes one key and signs with it.
pub struct Issuer {
    pub url: String,
    key: SigningKey,
}

fn seeded(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

impl Issuer {
    /// Serve `/.well-known/jwks.json` on a loopback port, for the life of
    /// the process.
    pub fn start() -> Issuer {
        let key = seeded(0x61);
        let kid = deployment_token::key_id(&key.verifying_key());
        let body =
            json!({"keys": [deployment_token::public_jwk(&kid, &key.verifying_key())]}).to_string();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind issuer");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            }
        });
        Issuer { url, key }
    }

    /// The settings the host binary needs to trust this issuer.
    pub fn settings(&self) -> Vec<(&'static str, String)> {
        vec![
            ("ORDO_IDENTITY_URL", self.url.clone()),
            ("ORDO_IDENTITY_ORIGIN", ISS.to_string()),
            ("ORDO_IDENTITY_DEPLOYMENT", HANDLE.to_string()),
        ]
    }

    fn claims(kind: Kind, touched_at: Option<u64>) -> DeploymentClaims {
        let now = now_secs();
        let (sub, client) = match kind {
            Kind::Person => ("p_matrix_ada", "console"),
            Kind::Machine => ("m_matrix_ci", "ci"),
            Kind::Breakglass => ("breakglass:root", "host"),
        };
        DeploymentClaims {
            iss: ISS.into(),
            aud: deployment_token::audience(HANDLE),
            sub: sub.into(),
            kind,
            role: ROLE_ADMIN.into(),
            client: client.into(),
            sid: "s_matrix".into(),
            touched_at,
            iat: now,
            exp: now + 4 * 3600,
            jti: "j_matrix".into(),
        }
    }

    fn kid(&self) -> String {
        deployment_token::key_id(&self.key.verifying_key())
    }

    fn sign(&self, c: &DeploymentClaims) -> String {
        deployment_token::sign(c, &self.kid(), &self.key)
    }

    fn touched() -> Option<u64> {
        Some(now_secs().saturating_sub(5))
    }

    /// A person who touched a moment ago.
    pub fn person_fresh(&self) -> String {
        self.sign(&Self::claims(Kind::Person, Self::touched()))
    }

    /// A person whose last touch is long past.
    pub fn person_stale(&self) -> String {
        self.sign(&Self::claims(
            Kind::Person,
            Some(now_secs().saturating_sub(STALE_SECS)),
        ))
    }

    /// A machine key's token: no touch.
    pub fn machine(&self) -> String {
        self.sign(&Self::claims(Kind::Machine, None))
    }

    /// Past its `exp` (beyond the leeway), everything else right.
    pub fn expired(&self) -> String {
        let mut c = Self::claims(Kind::Person, Self::touched());
        c.iat = now_secs().saturating_sub(7_200);
        c.exp = now_secs().saturating_sub(3_600);
        self.sign(&c)
    }

    /// Another deployment's `aud`.
    pub fn wrong_aud(&self) -> String {
        let mut c = Self::claims(Kind::Person, Self::touched());
        c.aud = deployment_token::audience("another");
        self.sign(&c)
    }

    /// Another issuer's `iss`.
    pub fn wrong_iss(&self) -> String {
        let mut c = Self::claims(Kind::Person, Self::touched());
        c.iss = "https://id.elsewhere.example".into();
        self.sign(&c)
    }

    /// Signed by a key the set does not hold, naming its own `kid`.
    pub fn unknown_kid(&self) -> String {
        let other = seeded(0x62);
        let c = Self::claims(Kind::Person, Self::touched());
        deployment_token::sign(
            &c,
            &deployment_token::key_id(&other.verifying_key()),
            &other,
        )
    }

    /// Signed by a key the set does not hold, naming the `kid` of the key
    /// it does.
    pub fn foreign_signer(&self) -> String {
        let other = seeded(0x63);
        let c = Self::claims(Kind::Person, Self::touched());
        deployment_token::sign(&c, &self.kid(), &other)
    }
}
