//! The deployment token: what the identity service signs for a deployment
//! person or machine, and what the door and the console verify
//! (`plan-identity-service.md` 4.1).
//!
//! A compact JWS (RFC 7515), 3 segments, `alg: EdDSA` over Ed25519 (RFC
//! 8037). The signature and the key choice are [`crate::jose`]'s, unchanged:
//! this module adds the claim set and the checks on it, so the service that
//! signs and every verifier read the same claims the same way.
//!
//! Header, exactly: `{"alg":"EdDSA","kid":"<kid>","typ":"ordo-deployment+jwt"}`.
//!
//! Claims, in this order: `iss`, `aud`, `sub`, `kind`, `role`, `client`,
//! `sid`, `touched_at` (absent on a machine token), `iat`, `exp`, `jti`.
//!
//! An app token (`crate::token`) is 2 segments with an HMAC and no `aud`,
//! so the realms cannot be confused: [`is_deployment_shape`] tells them
//! apart before either verifier runs.
//!
//! Every refusal is one error, [`DeploymentTokenError`], with a fixed word
//! safe for the security stream.

use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::jose::{self, Alg, Jwks};
use crate::webauthn::b64url_encode;

/// The header's `typ`.
pub const TYP: &str = "ordo-deployment+jwt";
/// What every `aud` starts with.
pub const AUD_PREFIX: &str = "ordo-deployment:";
/// The only role there is for now (`plan-identity-service.md` 11).
pub const ROLE_ADMIN: &str = "admin";
/// How far a verifier's clock may sit behind the signer's, for `iat`.
pub const DEFAULT_LEEWAY_SECS: u64 = 30;

/// The `aud` of a deployment's tokens: `ordo-deployment:<handle>`.
pub fn audience(handle: &str) -> String {
    format!("{AUD_PREFIX}{handle}")
}

/// A deployment token is 3 segments; an app token is 2. Decided on shape
/// alone, so neither verifier ever runs on the other's string.
pub fn is_deployment_shape(token: &str) -> bool {
    token.split('.').count() == 3
}

/// Who a token is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A deployment person, `sub` `p_…`.
    Person,
    /// A machine key, `sub` `m_…`. Never carries `touched_at`.
    Machine,
    /// Break-glass from the host, `sub` `breakglass:<unix user>`.
    Breakglass,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Person => "person",
            Kind::Machine => "machine",
            Kind::Breakglass => "breakglass",
        }
    }

    pub fn parse(word: &str) -> Option<Kind> {
        match word {
            "person" => Some(Kind::Person),
            "machine" => Some(Kind::Machine),
            "breakglass" => Some(Kind::Breakglass),
            _ => None,
        }
    }

    /// The `sub` this kind carries starts with this.
    fn sub_prefix(self) -> &'static str {
        match self {
            Kind::Person => "p_",
            Kind::Machine => "m_",
            Kind::Breakglass => "breakglass:",
        }
    }
}

/// The claim set, as signed and as verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentClaims {
    /// `ORDO_IDENTITY_ORIGIN`.
    pub iss: String,
    /// `ordo-deployment:<handle>`.
    pub aud: String,
    pub sub: String,
    pub kind: Kind,
    /// `admin`, the only role for now.
    pub role: String,
    /// `console`, `cli`, or the machine key's name. Audit only.
    pub client: String,
    /// The session id, so the audit joins sessions.
    pub sid: String,
    /// The last passkey assertion in the session, in seconds. Absent on a
    /// machine token. The door maps it to `user_presence_at`.
    pub touched_at: Option<u64>,
    pub iat: u64,
    pub exp: u64,
    pub jti: String,
}

impl DeploymentClaims {
    /// The payload, members in the order the module header lists.
    pub fn to_json(&self) -> String {
        let mut members: Vec<(&str, Value)> = vec![
            ("iss", Value::from(self.iss.as_str())),
            ("aud", Value::from(self.aud.as_str())),
            ("sub", Value::from(self.sub.as_str())),
            ("kind", Value::from(self.kind.as_str())),
            ("role", Value::from(self.role.as_str())),
            ("client", Value::from(self.client.as_str())),
            ("sid", Value::from(self.sid.as_str())),
        ];
        if let Some(t) = self.touched_at {
            members.push(("touched_at", Value::from(t)));
        }
        members.push(("iat", Value::from(self.iat)));
        members.push(("exp", Value::from(self.exp)));
        members.push(("jti", Value::from(self.jti.as_str())));
        // Written member by member, so the order holds whatever map the
        // build of `serde_json` keeps.
        let body: Vec<String> = members
            .iter()
            .map(|(k, v)| format!("{}:{v}", Value::from(*k)))
            .collect();
        format!("{{{}}}", body.join(","))
    }

    fn from_map(m: &Map<String, Value>) -> Result<DeploymentClaims, DeploymentTokenError> {
        let text = |k: &'static str| -> Result<String, DeploymentTokenError> {
            m.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or(DeploymentTokenError::Malformed(k))
        };
        let secs = |k: &'static str| -> Result<u64, DeploymentTokenError> {
            m.get(k)
                .and_then(Value::as_u64)
                .ok_or(DeploymentTokenError::Malformed(k))
        };
        let kind = Kind::parse(&text("kind")?).ok_or(DeploymentTokenError::Malformed("kind"))?;
        let sub = text("sub")?;
        if !sub.starts_with(kind.sub_prefix()) || sub.len() == kind.sub_prefix().len() {
            return Err(DeploymentTokenError::Malformed("sub"));
        }
        let role = text("role")?;
        if role != ROLE_ADMIN {
            return Err(DeploymentTokenError::Malformed("role"));
        }
        let touched_at = match m.get("touched_at") {
            None => None,
            Some(v) => Some(
                v.as_u64()
                    .ok_or(DeploymentTokenError::Malformed("touched_at"))?,
            ),
        };
        if kind == Kind::Machine && touched_at.is_some() {
            return Err(DeploymentTokenError::Malformed("touched_at"));
        }
        Ok(DeploymentClaims {
            iss: text("iss")?,
            aud: text("aud")?,
            sub,
            kind,
            role,
            client: text("client")?,
            sid: text("sid")?,
            touched_at,
            iat: secs("iat")?,
            exp: secs("exp")?,
            jti: text("jti")?,
        })
    }
}

/// Why a deployment token was refused. One error for every refusal; the
/// word is safe for the security stream and carries no byte of the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeploymentTokenError {
    /// The signature, the key or the serialisation was refused
    /// ([`jose::JoseError`]): an unknown or retired `kid` lands here.
    Unverifiable(jose::JoseError),
    /// `iss` is not the identity service this verifier trusts.
    WrongIssuer,
    /// `aud` is not this deployment.
    WrongAudience,
    /// `exp` has passed.
    Expired,
    /// `iat` is ahead of this verifier's clock by more than the leeway.
    NotYetValid,
    /// A claim is missing or has the wrong shape. Names the claim.
    Malformed(&'static str),
}

impl DeploymentTokenError {
    /// The fixed word.
    pub fn word(&self) -> &'static str {
        match self {
            DeploymentTokenError::Unverifiable(e) => e.0,
            DeploymentTokenError::WrongIssuer => "wrong-issuer",
            DeploymentTokenError::WrongAudience => "wrong-audience",
            DeploymentTokenError::Expired => "expired",
            DeploymentTokenError::NotYetValid => "not-yet-valid",
            DeploymentTokenError::Malformed(_) => "malformed-claims",
        }
    }
}

impl std::fmt::Display for DeploymentTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeploymentTokenError::Malformed(claim) => {
                write!(
                    f,
                    "deployment token refused: claim `{claim}` is missing or malformed"
                )
            }
            other => write!(f, "deployment token refused: {}", other.word()),
        }
    }
}

impl std::error::Error for DeploymentTokenError {}

/// What a verifier expects of a token.
#[derive(Debug, Clone)]
pub struct Expect<'a> {
    /// The identity service's origin.
    pub issuer: &'a str,
    /// `ordo-deployment:<handle>`, from [`audience`].
    pub audience: &'a str,
    /// The verifier's clock, seconds.
    pub now: u64,
    /// How far ahead `iat` may be.
    pub leeway_secs: u64,
}

/// Verify a deployment token against the published keys: signature and
/// `kid` first (`jose`), then `iss`, `aud`, `exp`, `iat`, then the shape of
/// every claim.
pub fn verify(
    token: &str,
    jwks: &Jwks,
    expect: &Expect<'_>,
) -> Result<DeploymentClaims, DeploymentTokenError> {
    if !is_deployment_shape(token) {
        return Err(DeploymentTokenError::Unverifiable(jose::JoseError(
            "not-compact-jws",
        )));
    }
    let verified = jose::verify_with_jwks(token, jwks, &[Alg::EdDSA])
        .map_err(DeploymentTokenError::Unverifiable)?;
    let map = verified
        .claims()
        .map_err(DeploymentTokenError::Unverifiable)?;
    let claims = DeploymentClaims::from_map(&map)?;
    if claims.iss != expect.issuer {
        return Err(DeploymentTokenError::WrongIssuer);
    }
    if claims.aud != expect.audience {
        return Err(DeploymentTokenError::WrongAudience);
    }
    if expect.now >= claims.exp {
        return Err(DeploymentTokenError::Expired);
    }
    if claims.iat > expect.now.saturating_add(expect.leeway_secs) {
        return Err(DeploymentTokenError::NotYetValid);
    }
    Ok(claims)
}

/// The key id of an Ed25519 public key: its RFC 7638 JWK thumbprint,
/// base64url. The same key always has the same id, on every node.
pub fn key_id(public: &VerifyingKey) -> String {
    let canonical = format!(
        r#"{{"crv":"Ed25519","kty":"OKP","x":"{}"}}"#,
        b64url_encode(public.as_bytes())
    );
    b64url_encode(&Sha256::digest(canonical.as_bytes()))
}

/// The JWK that publishes a token key: `kty`, `crv`, `x`, `kid`, `use`,
/// `alg`.
pub fn public_jwk(kid: &str, public: &VerifyingKey) -> Value {
    serde_json::json!({
        "kty": "OKP",
        "crv": "Ed25519",
        "x": b64url_encode(public.as_bytes()),
        "kid": kid,
        "use": "sig",
        "alg": "EdDSA",
    })
}

/// Sign a claim set under `kid`. The service's one signer; tests in other
/// crates use it to stand a fake issuer up.
pub fn sign(claims: &DeploymentClaims, kid: &str, key: &SigningKey) -> String {
    let header = format!(
        r#"{{"alg":"EdDSA","kid":{},"typ":"{TYP}"}}"#,
        Value::from(kid)
    );
    let input = format!(
        "{}.{}",
        b64url_encode(header.as_bytes()),
        b64url_encode(claims.to_json().as_bytes())
    );
    let sig = key.sign(input.as_bytes());
    format!("{input}.{}", b64url_encode(&sig.to_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISS: &str = "https://id.example.com";

    fn claims(now: u64) -> DeploymentClaims {
        DeploymentClaims {
            iss: ISS.into(),
            aud: audience("main"),
            sub: "p_0123".into(),
            kind: Kind::Person,
            role: ROLE_ADMIN.into(),
            client: "console".into(),
            sid: "s_1".into(),
            touched_at: Some(now - 5),
            iat: now,
            exp: now + 600,
            jti: "j1".into(),
        }
    }

    fn set(keys: &[(&str, &SigningKey)]) -> Jwks {
        let keys: Vec<Value> = keys
            .iter()
            .map(|(kid, k)| public_jwk(kid, &k.verifying_key()))
            .collect();
        Jwks::parse(serde_json::json!({ "keys": keys }).to_string().as_bytes()).unwrap()
    }

    fn expect(now: u64) -> Expect<'static> {
        Expect {
            issuer: ISS,
            audience: "ordo-deployment:main",
            now,
            leeway_secs: DEFAULT_LEEWAY_SECS,
        }
    }

    #[test]
    fn a_signed_token_verifies_and_reads_back_the_same_claims() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let kid = key_id(&key.verifying_key());
        let c = claims(1_000);
        let token = sign(&c, &kid, &key);
        assert!(is_deployment_shape(&token));
        assert_eq!(verify(&token, &set(&[(&kid, &key)]), &expect(1_000)), Ok(c));
    }

    #[test]
    fn issuer_audience_expiry_and_kid_are_each_refused() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let kid = key_id(&key.verifying_key());
        let jwks = set(&[(&kid, &key)]);
        let token = sign(&claims(1_000), &kid, &key);
        let mut e = expect(1_000);
        e.issuer = "https://other.example.com";
        assert_eq!(
            verify(&token, &jwks, &e),
            Err(DeploymentTokenError::WrongIssuer)
        );
        let mut e = expect(1_000);
        e.audience = "ordo-deployment:other";
        assert_eq!(
            verify(&token, &jwks, &e),
            Err(DeploymentTokenError::WrongAudience)
        );
        assert_eq!(
            verify(&token, &jwks, &expect(1_600)),
            Err(DeploymentTokenError::Expired)
        );
        assert_eq!(
            verify(&token, &jwks, &expect(900)),
            Err(DeploymentTokenError::NotYetValid)
        );
        let other = SigningKey::from_bytes(&[8; 32]);
        let other_kid = key_id(&other.verifying_key());
        assert_eq!(
            verify(&token, &set(&[(&other_kid, &other)]), &expect(1_000))
                .unwrap_err()
                .word(),
            "kid-unknown"
        );
        // A forged signature under the right kid.
        let forged = sign(&claims(1_000), &kid, &other);
        assert_eq!(
            verify(&forged, &jwks, &expect(1_000)).unwrap_err().word(),
            "signature"
        );
    }

    #[test]
    fn an_app_token_shape_never_reaches_the_jws_path() {
        assert!(!is_deployment_shape("abc.def"));
        let jwks = set(&[("k", &SigningKey::from_bytes(&[7; 32]))]);
        assert_eq!(
            verify("abc.def", &jwks, &expect(1)).unwrap_err().word(),
            "not-compact-jws"
        );
    }

    #[test]
    fn a_machine_token_with_a_touch_or_a_sub_of_another_kind_is_malformed() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let kid = key_id(&key.verifying_key());
        let jwks = set(&[(&kid, &key)]);
        let mut c = claims(1_000);
        c.kind = Kind::Machine;
        c.sub = "m_ci".into();
        assert_eq!(
            verify(&sign(&c, &kid, &key), &jwks, &expect(1_000)),
            Err(DeploymentTokenError::Malformed("touched_at"))
        );
        c.touched_at = None;
        assert!(verify(&sign(&c, &kid, &key), &jwks, &expect(1_000)).is_ok());
        c.sub = "p_someone".into();
        assert_eq!(
            verify(&sign(&c, &kid, &key), &jwks, &expect(1_000)),
            Err(DeploymentTokenError::Malformed("sub"))
        );
    }

    #[test]
    fn the_key_id_is_the_rfc_7638_thumbprint() {
        // RFC 8037 appendix A.3: the thumbprint of its Ed25519 example key.
        let x =
            crate::webauthn::b64url_decode("11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo").unwrap();
        let public = VerifyingKey::from_bytes(&x.try_into().unwrap()).unwrap();
        assert_eq!(
            key_id(&public),
            "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k"
        );
    }
}
