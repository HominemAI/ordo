//! Linking a console to a stand-in door by sign-in, the one link path left
//! for a console without the identity service (owner, 2026-10-30: the
//! claim step is removed).
//!
//! The deployment owner registers a passkey at the deployment like anyone,
//! and the deployment names them its owner (`ORDO_DEPLOYMENT_OWNERS` and a
//! restart, at a real door). [`register`] is the first half against the
//! door's real auth routes; the test then names the principal it answers
//! as the stand-in's owner. The console links with `kind: sign-in` and the
//! passkey's [`assertion`].

#![allow(dead_code)]

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{CompleteRequest, PrincipalId, TokenKey};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn post(router: &Router, path: &str, body: Value) -> Value {
    let res = router
        .clone()
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert!(status.is_success(), "{path}: {status} {v}");
    v
}

/// Register `device` at the door's auth routes; answers its principal.
pub async fn register(
    auth_router: &Router,
    device: &TestAuthenticator,
    secret: &[u8],
    at: u64,
) -> PrincipalId {
    let begun = post(
        auth_router,
        "/auth/register",
        json!({"method": "passkey", "display": "Deployment owner"}),
    )
    .await;
    let ch = begun["challenge"].as_str().expect("challenge").to_string();
    let CompleteRequest::PasskeyRegistration {
        client_data_json,
        attestation_object,
        ..
    } = device.register(&ch)
    else {
        unreachable!()
    };
    let done = post(
        auth_router,
        "/auth/complete",
        json!({"method": "passkey", "kind": "passkey_registration", "challenge": ch,
               "client_data_json": b64url_encode(&client_data_json),
               "attestation_object": b64url_encode(&attestation_object)}),
    )
    .await;
    let token = done["token"].as_str().expect("token");
    ordo_identity::verify(
        &TokenKey::from_secret(secret),
        token,
        ordo_identity::Timestamp(at),
    )
    .expect("a token of this door")
    .principal
}

/// The passkey body the console's `door/link/finish` takes for `sign-in`.
pub fn assertion(device: &TestAuthenticator, challenge: &str) -> Value {
    let CompleteRequest::PasskeyAssertion {
        challenge,
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
    } = device.assert(challenge)
    else {
        unreachable!()
    };
    json!({"kind": "passkey_assertion", "challenge": challenge.0,
           "credential_id": b64url_encode(&credential_id),
           "client_data_json": b64url_encode(&client_data_json),
           "authenticator_data": b64url_encode(&authenticator_data),
           "signature": b64url_encode(&signature)})
}
