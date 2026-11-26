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

#[cfg(test)]
mod tests {
    use super::*;

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
