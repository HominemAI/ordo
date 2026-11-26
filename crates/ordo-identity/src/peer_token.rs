//! Tokens one Ordo deployment uses to call another
//! (`plan-federation-grants.md` 5, step G2).
//!
//! The words: the **holder** holds the data, the **consumer** calls the
//! holder's door. Both are deployments registered at one Ordo identity
//! service.
//!
//! - The consumer proves itself at the service's token endpoint with a
//!   **client assertion** (RFC 7523, `private_key_jwt`): a short JWT it
//!   signs with its **consumer key**. The key is Ed25519 and is derived
//!   from the deployment's master key and its handle ([`consumer_key`]), so
//!   no person makes, copies or stores a private key. Only the public half
//!   is registered at the service.
//! - **On behalf of a person** (RFC 8693 token exchange): the consumer
//!   sends the person's ID token it holds and gets a token whose `sub` is
//!   the person, `aud` is the holder ([`audience`]) and `act.sub` is the
//!   consumer. Its header `typ` is [`TYP_ON_BEHALF`].
//! - **As itself** (`client_credentials`): `sub` is the consumer, `aud` the
//!   holder, no `act`. Its header `typ` is [`TYP_AS_ITSELF`] and its `kind`
//!   claim is `client`, so a verifier that wants a person never reads it as
//!   one.
//!
//! Neither token is a deployment token (`typ` `ordo-deployment+jwt`,
//! `aud` `ordo-deployment:<handle>`) or an ID token (`typ` `JWT`, `aud` the
//! bare handle), so the door's and an app's existing checks refuse both.

use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use serde_json::{json, Value};

use crate::webauthn::b64url_encode;

/// RFC 8693's grant type.
pub const GRANT_TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
/// RFC 6749's grant for a client acting as itself.
pub const GRANT_CLIENT_CREDENTIALS: &str = "client_credentials";
/// RFC 7523's client assertion type.
pub const ASSERTION_TYPE: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";
/// The subject token type the exchange takes: the person's ID token.
pub const TOKEN_TYPE_ID_TOKEN: &str = "urn:ietf:params:oauth:token-type:id_token";
/// The issued token type.
pub const TOKEN_TYPE_ACCESS_TOKEN: &str = "urn:ietf:params:oauth:token-type:access_token";
/// The header `typ` of a token on behalf of a person.
pub const TYP_ON_BEHALF: &str = "ordo-peer+jwt";
/// The header `typ` of a token for a consumer acting as itself.
pub const TYP_AS_ITSELF: &str = "ordo-client+jwt";
/// The `kind` claim of each.
pub const KIND_PERSON: &str = "person";
pub const KIND_CLIENT: &str = "client";
/// The subject token type of a consumer's own session statement (RFC 8693,
/// a JWT): what the consumer exchanges once the person's ID token has
/// expired (step G5, board `federation-build-5-choices-from-steps` ask 2).
pub const TOKEN_TYPE_JWT: &str = "urn:ietf:params:oauth:token-type:jwt";
/// The header `typ` of that statement.
pub const TYP_SESSION: &str = "ordo-session+jwt";

/// A consumer's statement that it holds a live session for `person`: `iss`
/// the consumer, `sub` the person (their id at the identity service), `aud`
/// the service's issuer, signed with the consumer key. The service takes it
/// as the subject of a token exchange from that consumer only, for a person
/// still a member of it, so the consumer keeps acting for a person past
/// their 10-minute ID token without holding any token of theirs.
pub fn session_subject(
    client_id: &str,
    issuer: &str,
    person: &str,
    key: &SigningKey,
    iat: u64,
    exp: u64,
) -> String {
    let claims = json!({
        "iss": client_id,
        "sub": person,
        "aud": issuer,
        "iat": iat,
        "exp": exp,
    });
    sign_jwt(&json!({ "alg": "EdDSA", "typ": TYP_SESSION }), &claims, key)
}

/// The `aud` a holder's tokens carry: `ordo-peer:<handle>`. Distinct from
/// a deployment token's and an ID token's, so neither is ever taken for
/// the other.
pub fn audience(holder: &str) -> String {
    format!("ordo-peer:{holder}")
}

/// The consumer key of the deployment `handle`, derived from its master
/// key: HMAC-SHA256 under the master key over a fixed label and the
/// handle, used as an Ed25519 seed. The same master key and handle give
/// the same key on every node, and 2 deployments that share a master key
/// still get 2 keys.
pub fn consumer_key(master: &[u8], handle: &str) -> SigningKey {
    let mut msg = b"ordo-consumer-key/v1\0".to_vec();
    msg.extend_from_slice(handle.as_bytes());
    SigningKey::from_bytes(&ordo_crypto::hmac_sha256(master, &msg))
}

/// The registered form of a consumer key's public half: base64url of its
/// 32 bytes.
pub fn public_text(public: &VerifyingKey) -> String {
    b64url_encode(public.as_bytes())
}

/// A client assertion (RFC 7523 2.2): `iss` and `sub` the consumer's
/// client id, `aud` the token endpoint, a fresh `jti`, issued at `iat`,
/// expiring at `exp`. The service takes each `jti` once.
pub fn client_assertion(
    client_id: &str,
    token_endpoint: &str,
    key: &SigningKey,
    iat: u64,
    exp: u64,
    jti: &str,
) -> String {
    let claims = json!({
        "iss": client_id,
        "sub": client_id,
        "aud": token_endpoint,
        "iat": iat,
        "exp": exp,
        "jti": jti,
    });
    sign_jwt(&json!({ "alg": "EdDSA", "typ": "JWT" }), &claims, key)
}

/// A compact JWS of `claims` under `header`, signed with `key`.
pub fn sign_jwt(header: &Value, claims: &Value, key: &SigningKey) -> String {
    let input = format!(
        "{}.{}",
        b64url_encode(header.to_string().as_bytes()),
        b64url_encode(claims.to_string().as_bytes())
    );
    let sig = key.sign(input.as_bytes());
    format!("{input}.{}", b64url_encode(&sig.to_bytes()))
}

// ── the call chain (`plan-federation-grants.md` 2 rule 9, 7) ─────────────

/// The claim that lists the deployments a call has passed through, the
/// consumer that asked for the token last. The service writes it, so it is
/// covered by the token's signature and cannot be stripped on the way.
pub const CHAIN_CLAIM: &str = "chain";
/// The token endpoint's form field a consumer sends its own incoming chain
/// in: the handles, space-separated, oldest first. Absent for a call that
/// starts at the consumer.
pub const CHAIN_PARAM: &str = "ordo_chain";
/// The longest chain a token may carry, the consumer included.
pub const MAX_CHAIN: usize = 8;

/// A deployment handle: lowercase letters, digits and dashes, at most 63.
pub fn valid_handle(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 63
        && h.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Read [`CHAIN_PARAM`]: handles separated by spaces or commas. `Err` says
/// what is wrong with it.
pub fn parse_chain(text: &str) -> Result<Vec<String>, String> {
    let chain: Vec<String> = text
        .split([' ', ','])
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if let Some(bad) = chain.iter().find(|h| !valid_handle(h)) {
        return Err(format!(
            "`{CHAIN_PARAM}` holds `{bad}`, which is not a deployment handle"
        ));
    }
    if chain.len() >= MAX_CHAIN {
        return Err(format!(
            "`{CHAIN_PARAM}` lists {} deployments; a call may pass through at most {MAX_CHAIN}",
            chain.len()
        ));
    }
    Ok(chain)
}

// ── verifying, at the holder (step G3) ───────────────────────────────────

/// The header `typ` of a compact JWS, unverified. Used only to tell the
/// token kinds apart before the right verifier runs.
pub fn typ_of(token: &str) -> Option<String> {
    let header = token.split('.').next()?;
    let bytes = crate::webauthn::b64url_decode(header).ok()?;
    match crate::jose::strict_json(&bytes).ok()? {
        Value::Object(m) => m.get("typ").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

/// Is this a token one deployment presents at another's door: 3 segments,
/// `typ` [`TYP_ON_BEHALF`] or [`TYP_AS_ITSELF`]? Decided before the
/// deployment-token verifier, which would refuse it anyway on `aud`.
pub fn is_peer_shape(token: &str) -> bool {
    token.split('.').count() == 3
        && matches!(
            typ_of(token).as_deref(),
            Some(TYP_ON_BEHALF | TYP_AS_ITSELF)
        )
}

/// Whom a token speaks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerKind {
    /// `sub` is a person; the consumer is `act.sub`.
    OnBehalf,
    /// `sub` is the consumer; no person.
    AsItself,
}

/// A verified token's claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerClaims {
    pub iss: String,
    pub aud: String,
    /// The person (on behalf) or the consumer (as itself), as the service
    /// names them.
    pub sub: String,
    /// The calling deployment's handle.
    pub consumer: String,
    pub kind: PeerKind,
    /// The deployments the call passed through, the consumer last. A token
    /// from a service that wrote no chain reads as the consumer alone.
    pub chain: Vec<String>,
    pub iat: u64,
    pub exp: u64,
    pub jti: String,
}

/// What a holder expects of a token.
pub struct Expect<'a> {
    /// The issuer of the holder's connection to the identity service.
    pub issuer: &'a str,
    /// [`audience`] of the holder's handle.
    pub audience: &'a str,
    pub now: u64,
    /// How far ahead `iat` may be.
    pub leeway_secs: u64,
}

/// Why a token was refused. Every variant after `Unverifiable` is about a
/// token whose signature verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerTokenError {
    /// Signature, key, or shape.
    Unverifiable(crate::jose::JoseError),
    /// The claims do not fit the header's `typ`: a missing `act`, a `kind`
    /// that disagrees, no `sub`.
    Malformed(&'static str),
    WrongIssuer(String),
    WrongAudience(String),
    Expired(u64),
    NotYetValid,
}

/// Verify a peer token against the identity service's keys: signature and
/// `kid`, `typ`, then `iss`, `aud`, `exp`, `iat`, and the claims each kind
/// must carry.
pub fn verify(
    token: &str,
    jwks: &crate::jose::Jwks,
    expect: &Expect<'_>,
) -> Result<PeerClaims, PeerTokenError> {
    use crate::jose::{self, Alg, JoseError};
    let kind = match typ_of(token).as_deref() {
        Some(TYP_ON_BEHALF) => PeerKind::OnBehalf,
        Some(TYP_AS_ITSELF) => PeerKind::AsItself,
        _ => return Err(PeerTokenError::Unverifiable(JoseError("not-a-peer-token"))),
    };
    let verified =
        jose::verify_with_jwks(token, jwks, &[Alg::EdDSA]).map_err(PeerTokenError::Unverifiable)?;
    let c = verified.claims().map_err(PeerTokenError::Unverifiable)?;
    let text = |k: &str| c.get(k).and_then(Value::as_str).map(str::to_string);
    let num = |k: &str| c.get(k).and_then(Value::as_u64);
    let sub = text("sub")
        .filter(|s| !s.is_empty())
        .ok_or(PeerTokenError::Malformed("no-sub"))?;
    let client_id = text("client_id").ok_or(PeerTokenError::Malformed("no-client-id"))?;
    let consumer = match kind {
        PeerKind::OnBehalf => {
            if text("kind").as_deref() != Some(KIND_PERSON) {
                return Err(PeerTokenError::Malformed("kind"));
            }
            let act = c
                .get("act")
                .and_then(|a| a.get("sub"))
                .and_then(Value::as_str)
                .ok_or(PeerTokenError::Malformed("no-act"))?;
            if act != client_id {
                return Err(PeerTokenError::Malformed("act-not-client"));
            }
            act.to_string()
        }
        PeerKind::AsItself => {
            if text("kind").as_deref() != Some(KIND_CLIENT) || c.contains_key("act") {
                return Err(PeerTokenError::Malformed("kind"));
            }
            if sub != client_id {
                return Err(PeerTokenError::Malformed("sub-not-client"));
            }
            client_id.clone()
        }
    };
    let iss = text("iss").unwrap_or_default();
    let aud = text("aud").unwrap_or_default();
    let exp = num("exp").ok_or(PeerTokenError::Malformed("no-exp"))?;
    let iat = num("iat").unwrap_or(0);
    let chain = match c.get(CHAIN_CLAIM) {
        None => vec![consumer.clone()],
        Some(Value::Array(list)) => {
            let chain: Option<Vec<String>> = list
                .iter()
                .map(|h| h.as_str().filter(|h| valid_handle(h)).map(str::to_string))
                .collect();
            match chain {
                Some(c) if !c.is_empty() && c.len() <= MAX_CHAIN && c.last() == Some(&consumer) => {
                    c
                }
                _ => return Err(PeerTokenError::Malformed("chain")),
            }
        }
        Some(_) => return Err(PeerTokenError::Malformed("chain")),
    };
    let trim = |s: &str| s.trim_end_matches('/').to_string();
    if trim(&iss) != trim(expect.issuer) {
        return Err(PeerTokenError::WrongIssuer(iss));
    }
    if aud != expect.audience {
        return Err(PeerTokenError::WrongAudience(aud));
    }
    if expect.now >= exp {
        return Err(PeerTokenError::Expired(exp));
    }
    if iat > expect.now.saturating_add(expect.leeway_secs) {
        return Err(PeerTokenError::NotYetValid);
    }
    Ok(PeerClaims {
        iss,
        aud,
        sub,
        consumer,
        kind,
        chain,
        iat,
        exp,
        jti: text("jti").unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_is_handles_and_bounded() {
        assert_eq!(parse_chain("a b,c").unwrap(), ["a", "b", "c"]);
        assert!(parse_chain("").unwrap().is_empty());
        assert!(parse_chain("Bad")
            .unwrap_err()
            .contains("not a deployment handle"));
        let long = ["x"; MAX_CHAIN].join(" ");
        assert!(parse_chain(&long).unwrap_err().contains("at most"));
    }

    #[test]
    fn a_peer_token_verifies_and_each_kind_must_fit_its_typ() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let kid = crate::deployment_token::key_id(&key.verifying_key());
        let jwks = crate::jose::Jwks::parse(
            json!({ "keys": [crate::deployment_token::public_jwk(&kid, &key.verifying_key())] })
                .to_string()
                .as_bytes(),
        )
        .unwrap();
        let expect = Expect {
            issuer: "https://ids.example",
            audience: "ordo-peer:notes",
            now: 1_000,
            leeway_secs: 30,
        };
        let sign = |typ: &str, claims: Value| {
            sign_jwt(
                &json!({ "alg": "EdDSA", "kid": kid, "typ": typ }),
                &claims,
                &key,
            )
        };
        let person = json!({
            "iss": "https://ids.example/", "sub": "p_ada", "aud": "ordo-peer:notes",
            "act": { "sub": "viewer" }, "client_id": "viewer", "kind": "person",
            "iat": 990, "exp": 1_200, "jti": "j", "chain": ["viewer"],
        });
        let t = sign(TYP_ON_BEHALF, person.clone());
        assert!(is_peer_shape(&t));
        let c = verify(&t, &jwks, &expect).unwrap();
        assert_eq!(
            (c.kind, c.consumer.as_str()),
            (PeerKind::OnBehalf, "viewer")
        );
        assert_eq!(c.chain, ["viewer"]);

        // The person's kind under the client's `typ` is refused.
        let t = sign(TYP_AS_ITSELF, person.clone());
        assert_eq!(
            verify(&t, &jwks, &expect),
            Err(PeerTokenError::Malformed("kind"))
        );
        // A chain that does not end with the consumer is refused.
        let mut forged = person.clone();
        forged["chain"] = json!(["viewer", "notes"]);
        let t = sign(TYP_ON_BEHALF, forged);
        assert_eq!(
            verify(&t, &jwks, &expect),
            Err(PeerTokenError::Malformed("chain"))
        );
        // Another audience, and expiry.
        let mut other = person.clone();
        other["aud"] = json!("ordo-peer:hal");
        let t = sign(TYP_ON_BEHALF, other);
        assert_eq!(
            verify(&t, &jwks, &expect),
            Err(PeerTokenError::WrongAudience("ordo-peer:hal".into()))
        );
        let late = Expect {
            now: 1_200,
            ..expect
        };
        let t = sign(TYP_ON_BEHALF, person);
        assert_eq!(
            verify(&t, &jwks, &late),
            Err(PeerTokenError::Expired(1_200))
        );
    }

    #[test]
    fn a_consumer_key_is_fixed_by_the_master_key_and_the_handle() {
        let a = consumer_key(&[7; 32], "notes");
        assert_eq!(
            a.verifying_key(),
            consumer_key(&[7; 32], "notes").verifying_key()
        );
        assert_ne!(
            a.verifying_key(),
            consumer_key(&[7; 32], "viewer").verifying_key()
        );
        assert_ne!(
            a.verifying_key(),
            consumer_key(&[8; 32], "notes").verifying_key()
        );
        assert_eq!(public_text(&a.verifying_key()).len(), 43);
    }

    #[test]
    fn the_audience_is_not_a_deployment_tokens() {
        assert_eq!(audience("notes"), "ordo-peer:notes");
        assert_ne!(
            audience("notes"),
            crate::deployment_token::audience("notes")
        );
    }
}
