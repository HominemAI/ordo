//! Attacks on the passkey (WebAuthn) ceremonies, replayed through the real
//! routes with the software authenticator (security round, 2026-10-25).
//!
//! One test per attack, named for what it tries. Each expects the refusal
//! (or the documented safe answer). A software authenticator can lie about
//! one thing at a time, which is what these use.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{AccessChecker, AccessDecision};
use ordo_host::auth::AuthState;
use ordo_host::metrics::Metrics;
use ordo_host::IdentityAccess;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::provider::{Argon2idHasher, CompleteRequest, LocalPasswordProvider};
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity::webauthn::{b64url_encode, signed_message};
use ordo_identity::{
    AccessEngine, AccessProfile, DirectoryStore, MemorySessionRevocations, PasskeyProvider,
    PrincipalId, PrincipalKind, ProviderRegistry, RelyingParty, Role, SpaceId, TokenKey,
    UserVerification,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &[u8] = b"passkey-attacks-test-secret";
const NOW: u64 = 1_000;
const SPACE: &str = "spc_clinic";
const GATED: &str = "transfer.approve";
const PLAIN: &str = "txns.list";

struct World {
    auth: Arc<AuthState>,
    keys: Arc<ordo_identity::MemoryPasskeyStore>,
    store: Arc<ordo_identity::MemoryDirectoryStore>,
    clock: Arc<std::sync::atomic::AtomicU64>,
    revocations: Arc<MemorySessionRevocations>,
}

impl World {
    fn set_now(&self, at: u64) {
        self.clock.store(at, std::sync::atomic::Ordering::SeqCst);
    }
    fn app(&self) -> axum::Router {
        ordo_host::auth::router(Some(self.auth.clone()))
    }
}

fn world_with(rp: RelyingParty) -> World {
    let keys = Arc::new(ordo_identity::MemoryPasskeyStore::new());
    let passkey = Arc::new(PasskeyProvider::with_stores(
        rp,
        keys.clone(),
        Arc::new(ordo_identity::MemoryCeremonyStore::new()),
    ));
    let password = LocalPasswordProvider::new(Argon2idHasher::weak_for_tests());
    password
        .enroll("ana@example.test", "correct-horse")
        .expect("enrolled");
    let mut registry = ProviderRegistry::new();
    registry
        .register("password", Box::new(password))
        .expect("password");
    registry
        .register("passkey", Box::new(passkey.clone()))
        .expect("passkey");
    let revocations = Arc::new(MemorySessionRevocations::new());
    let store = Arc::new(ordo_identity::MemoryDirectoryStore::new());
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, store.clone()),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations.clone())
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()))
    .with_passkey(passkey);
    let clock = Arc::new(std::sync::atomic::AtomicU64::new(NOW));
    let read = clock.clone();
    state.now = Box::new(move || read.load(std::sync::atomic::Ordering::SeqCst));
    World {
        auth: Arc::new(state),
        keys,
        store,
        clock,
        revocations,
    }
}

fn world() -> World {
    world_with(RelyingParty::new("example.com", &["https://app.example.com"]).expect("rp"))
}

async fn send(
    app: &axum::Router,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method("POST").uri(path);
    if body.is_some() {
        b = b.header("content-type", "application/json");
    }
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let res = app
        .clone()
        .oneshot(
            b.body(match body {
                Some(v) => Body::from(v.to_string()),
                None => Body::empty(),
            })
            .expect("request"),
        )
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn post(app: &axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    send(app, path, Some(body), None).await
}

fn assertion_json(
    challenge: &str,
    credential_id: &[u8],
    client_data: &[u8],
    auth_data: &[u8],
    signature: &[u8],
) -> Value {
    json!({
        "method": "passkey", "kind": "passkey_assertion", "challenge": challenge,
        "credential_id": b64url_encode(credential_id),
        "client_data_json": b64url_encode(client_data),
        "authenticator_data": b64url_encode(auth_data),
        "signature": b64url_encode(signature),
        "display": "Ana",
    })
}

fn assertion_body(req: &CompleteRequest) -> Value {
    let CompleteRequest::PasskeyAssertion {
        challenge,
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
    } = req
    else {
        panic!("not an assertion");
    };
    assertion_json(
        &challenge.0,
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
    )
}

fn registration_json(challenge: &str, client_data: &[u8], attestation: &[u8]) -> Value {
    json!({
        "method": "passkey", "kind": "passkey_registration", "challenge": challenge,
        "client_data_json": b64url_encode(client_data),
        "attestation_object": b64url_encode(attestation),
        "display": "Mallory",
    })
}

fn registration_body(req: &CompleteRequest) -> Value {
    let CompleteRequest::PasskeyRegistration {
        challenge,
        client_data_json,
        attestation_object,
    } = req
    else {
        panic!("not a registration");
    };
    registration_json(&challenge.0, client_data_json, attestation_object)
}

/// An assertion with every part chosen by the attacker. `signer` signs, and
/// `holder` names the credential id presented.
fn forged(
    holder: &TestAuthenticator,
    signer: &TestAuthenticator,
    challenge: &str,
    ty: &str,
    origin: &str,
    auth_data: &[u8],
) -> Value {
    let cd = holder.client_data(ty, challenge, origin, false);
    let sig = signer.key.sign(&signed_message(auth_data, &cd));
    assertion_json(challenge, &holder.credential_id, &cd, auth_data, &sig)
}

async fn begin_sign_in(app: &axum::Router) -> String {
    let (status, o) = post(app, "/auth/begin", json!({"method": "passkey"})).await;
    assert_eq!(status, StatusCode::OK, "{o}");
    o["challenge"].as_str().expect("challenge").to_string()
}

async fn begin_register(app: &axum::Router) -> String {
    let (status, o) = post(app, "/auth/register", json!({"method": "passkey"})).await;
    assert_eq!(status, StatusCode::OK, "{o}");
    o["challenge"].as_str().expect("challenge").to_string()
}

/// Register `device` through the routes: a person with that passkey.
async fn sign_up(app: &axum::Router, device: &TestAuthenticator) -> String {
    let ch = begin_register(app).await;
    let (status, done) = post(
        app,
        "/auth/complete",
        registration_body(&device.register(&ch)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done["token"].as_str().expect("token").to_string()
}

async fn sign_in(app: &axum::Router, device: &TestAuthenticator) -> (StatusCode, Value) {
    let ch = begin_sign_in(app).await;
    post(app, "/auth/complete", assertion_body(&device.assert(&ch))).await
}

fn enroll_directly(w: &World, device: &TestAuthenticator) {
    let now = ordo_identity::Timestamp(NOW);
    let passkey = w.auth.passkey.as_ref().expect("passkey").clone();
    let options = passkey.begin_registration(now, &[]).expect("options");
    ordo_identity::AuthProvider::complete(&passkey, &device.register(&options.challenge), now)
        .expect("registers");
}

fn principal_of(token: &str) -> PrincipalId {
    ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        token,
        ordo_identity::Timestamp(NOW),
    )
    .expect("a token this deployment minted")
    .principal
}

fn door_for(principal: &PrincipalId, w: &World, at: u64) -> IdentityAccess {
    let space = SpaceId(SPACE.into());
    let mut engine = AccessEngine::new();
    engine.set_profile(
        &space,
        AccessProfile::new()
            .allow(Role::Owner, PLAIN)
            .allow(Role::Owner, GATED),
    );
    engine.set_membership(&space, principal, Role::Owner);
    engine.require_step_up(GATED);
    IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(move || at),
        Arc::new(Metrics::new()),
    )
    .with_revocations(w.revocations.clone())
}

fn link(w: &World, device: &TestAuthenticator, p: &ordo_identity::Principal, not_before: u64) {
    if not_before == 0 {
        w.store
            .insert_link(&device.authenticator_id(), p, ordo_identity::Timestamp(NOW))
            .expect("linked");
    } else {
        w.store
            .insert_link_marked(
                &device.authenticator_id(),
                p,
                ordo_identity::Timestamp(NOW),
                not_before,
                "case-1",
            )
            .expect("linked, quarantined");
    }
}

/// A person with a password and `device` as a passkey (quarantined until
/// `not_before` when non-zero), signed in with the password: no presence.
async fn password_session(
    w: &World,
    app: &axum::Router,
    device: &TestAuthenticator,
    not_before: u64,
) -> (String, PrincipalId) {
    let principal = w
        .store
        .create_principal(PrincipalKind::Human, "Ana", ordo_identity::Timestamp(NOW))
        .expect("minted");
    let p = w.store.principal(&principal).expect("read").expect("some");
    w.store
        .insert_link(
            &ordo_identity::AuthenticatorId("password:ana@example.test".into()),
            &p,
            ordo_identity::Timestamp(NOW),
        )
        .expect("linked");
    link(w, device, &p, not_before);
    enroll_directly(w, device);
    let (_, begun) = post(
        app,
        "/auth/begin",
        json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let (status, done) = post(
        app,
        "/auth/complete",
        json!({"method": "password", "kind": "password",
               "challenge": begun["challenge"], "identifier": "ana@example.test",
               "password": "correct-horse"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    (
        done["token"].as_str().expect("token").to_string(),
        principal,
    )
}

async fn step_up_challenge(app: &axum::Router, token: &str) -> String {
    let (status, o) = send(app, "/auth/step-up/begin", None, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{o}");
    o["challenge"].as_str().expect("challenge").to_string()
}

async fn step_up(app: &axum::Router, token: &str, body: Value) -> (StatusCode, Value) {
    send(app, "/auth/step-up", Some(body), Some(token)).await
}

fn refused(r: &(StatusCode, Value)) {
    assert_eq!(r.0, StatusCode::UNAUTHORIZED, "{}", r.1);
    assert!(r.1.get("token").is_none(), "no token on a refusal: {}", r.1);
}

fn good_auth_data(d: &TestAuthenticator) -> Vec<u8> {
    d.auth_data(false)
}

// ═══════════════════════ challenges: replay, borrow, expiry ═══════════════

#[tokio::test(flavor = "multi_thread")]
async fn replays_a_sign_in_assertion_after_it_was_spent() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    let ch = begin_sign_in(&app).await;
    let body = assertion_body(&d.assert(&ch));
    assert_eq!(
        post(&app, "/auth/complete", body.clone()).await.0,
        StatusCode::OK
    );
    refused(&post(&app, "/auth/complete", body).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn replays_a_registration_after_its_challenge_was_spent() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    let ch = begin_register(&app).await;
    let body = registration_body(&d.register(&ch));
    assert_eq!(
        post(&app, "/auth/complete", body.clone()).await.0,
        StatusCode::OK
    );
    refused(&post(&app, "/auth/complete", body).await);
    assert_eq!(w.keys.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_a_challenge_the_server_never_issued() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    refused(
        &post(
            &app,
            "/auth/complete",
            assertion_body(&d.assert("c2VydmVyLW5ldmVyLWlzc3VlZA")),
        )
        .await,
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn signs_a_challenge_issued_to_another_ceremony_inside_the_client_data() {
    // Mallory holds a live challenge of her own, but her client data names
    // the victim's challenge text.
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    let mine = begin_sign_in(&app).await;
    let theirs = begin_sign_in(&app).await;
    let cd = d.client_data("webauthn.get", &theirs, &d.origin, false);
    let ad = good_auth_data(&d);
    let sig = d.key.sign(&signed_message(&ad, &cd));
    refused(
        &post(
            &app,
            "/auth/complete",
            assertion_json(&mine, &d.credential_id, &cd, &ad, &sig),
        )
        .await,
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_another_sessions_step_up_challenge_on_my_session() {
    let w = world();
    let app = w.app();
    let mine = TestAuthenticator::es256();
    let (my_token, _) = password_session(&w, &app, &mine, 0).await;
    let theirs = TestAuthenticator::ed25519();
    let their_token = sign_up(&app, &theirs).await;
    // Their challenge, answered by my own passkey on my session.
    let ch = step_up_challenge(&app, &their_token).await;
    refused(&step_up(&app, &my_token, assertion_body(&mine.assert(&ch))).await);
    // And on their session, answered by my passkey.
    let ch = step_up_challenge(&app, &their_token).await;
    refused(&step_up(&app, &their_token, assertion_body(&mine.assert(&ch))).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_a_step_up_challenge_at_the_sign_in_route() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    let (token, _) = password_session(&w, &app, &d, 0).await;
    let ch = step_up_challenge(&app, &token).await;
    refused(&post(&app, "/auth/complete", assertion_body(&d.assert(&ch))).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_a_sign_in_challenge_at_the_step_up_route() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    let (token, _) = password_session(&w, &app, &d, 0).await;
    let ch = begin_sign_in(&app).await;
    refused(&step_up(&app, &token, assertion_body(&d.assert(&ch))).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_a_sign_in_challenge_past_its_expiry() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    let ch = begin_sign_in(&app).await;
    w.set_now(NOW + 301);
    refused(&post(&app, "/auth/complete", assertion_body(&d.assert(&ch))).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_a_registration_challenge_past_its_expiry() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    let ch = begin_register(&app).await;
    w.set_now(NOW + 301);
    refused(&post(&app, "/auth/complete", registration_body(&d.register(&ch))).await);
    assert_eq!(w.keys.len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_a_step_up_challenge_past_its_expiry() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    let (token, _) = password_session(&w, &app, &d, 0).await;
    let ch = step_up_challenge(&app, &token).await;
    w.set_now(NOW + 301);
    refused(&step_up(&app, &token, assertion_body(&d.assert(&ch))).await);
}

// ═══════════════════════ client data and authenticator data ═══════════════

#[tokio::test(flavor = "multi_thread")]
async fn asserts_from_a_wrong_origin() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    for origin in [
        "https://evil.example.net",
        "https://app.example.com.evil.net",
        "http://app.example.com",
        "https://app.example.com:8443",
    ] {
        let ch = begin_sign_in(&app).await;
        refused(
            &post(
                &app,
                "/auth/complete",
                assertion_body(&d.assert_at(&ch, origin)),
            )
            .await,
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn registers_from_a_wrong_origin() {
    let w = world();
    let app = w.app();
    let mut d = TestAuthenticator::es256();
    d.origin = "https://evil.example.net".into();
    let ch = begin_register(&app).await;
    refused(&post(&app, "/auth/complete", registration_body(&d.register(&ch))).await);
    assert_eq!(w.keys.len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn asserts_in_a_cross_origin_frame() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    let ch = begin_sign_in(&app).await;
    let cd = d.client_data("webauthn.get", &ch, &d.origin, true);
    let ad = good_auth_data(&d);
    let sig = d.key.sign(&signed_message(&ad, &cd));
    refused(
        &post(
            &app,
            "/auth/complete",
            assertion_json(&ch, &d.credential_id, &cd, &ad, &sig),
        )
        .await,
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn asserts_for_a_wrong_rp_id_hash() {
    let w = world();
    let app = w.app();
    let mut d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    d.rp_id = "evil.example.net".into();
    let ch = begin_sign_in(&app).await;
    refused(&post(&app, "/auth/complete", assertion_body(&d.assert(&ch))).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn registers_for_a_wrong_rp_id_hash() {
    let w = world();
    let app = w.app();
    let mut d = TestAuthenticator::es256();
    d.rp_id = "evil.example.net".into();
    let ch = begin_register(&app).await;
    refused(&post(&app, "/auth/complete", registration_body(&d.register(&ch))).await);
    assert_eq!(w.keys.len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn asserts_with_user_presence_cleared() {
    let w = world();
    let app = w.app();
    let mut d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    d.user_present = false;
    refused(&sign_in(&app, &d).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn asserts_with_user_verification_cleared_where_it_is_required() {
    // The default deployment requires user verification.
    let w = world();
    let app = w.app();
    let mut d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    d.user_verified = false;
    refused(&sign_in(&app, &d).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn registers_with_user_presence_or_verification_cleared() {
    let w = world();
    let app = w.app();
    let mut d = TestAuthenticator::es256();
    d.user_present = false;
    let ch = begin_register(&app).await;
    refused(&post(&app, "/auth/complete", registration_body(&d.register(&ch))).await);
    let mut d = TestAuthenticator::es256();
    d.user_verified = false;
    let ch = begin_register(&app).await;
    refused(&post(&app, "/auth/complete", registration_body(&d.register(&ch))).await);
    assert_eq!(w.keys.len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn steps_up_with_user_verification_cleared_where_it_is_required() {
    let w = world();
    let app = w.app();
    let mut d = TestAuthenticator::es256();
    let (token, principal) = password_session(&w, &app, &d, 0).await;
    d.user_verified = false;
    let ch = step_up_challenge(&app, &token).await;
    refused(&step_up(&app, &token, assertion_body(&d.assert(&ch))).await);
    let door = door_for(&principal, &w, NOW + 10);
    assert_eq!(door.check(&token, SPACE, GATED), AccessDecision::Deny);
}

#[tokio::test(flavor = "multi_thread")]
async fn steps_up_with_user_presence_cleared() {
    let w = world();
    let app = w.app();
    let mut d = TestAuthenticator::es256();
    let (token, _) = password_session(&w, &app, &d, 0).await;
    d.user_present = false;
    let ch = step_up_challenge(&app, &token).await;
    refused(&step_up(&app, &token, assertion_body(&d.assert(&ch))).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn asserts_with_a_get_type_where_a_create_is_owed() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    let ch = begin_register(&app).await;
    let cd = d.client_data("webauthn.get", &ch, &d.origin, false);
    refused(
        &post(
            &app,
            "/auth/complete",
            registration_json(&ch, &cd, &d.attestation_object("none")),
        )
        .await,
    );
    assert_eq!(w.keys.len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn asserts_with_a_create_type_where_a_get_is_owed() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    let ch = begin_sign_in(&app).await;
    let ad = good_auth_data(&d);
    refused(
        &post(
            &app,
            "/auth/complete",
            forged(&d, &d, &ch, "webauthn.create", &d.origin, &ad),
        )
        .await,
    );
    let ch = begin_sign_in(&app).await;
    refused(
        &post(
            &app,
            "/auth/complete",
            forged(&d, &d, &ch, "", &d.origin, &ad),
        )
        .await,
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn registers_with_an_unsupported_attestation_format() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    let ch = begin_register(&app).await;
    let cd = d.client_data("webauthn.create", &ch, &d.origin, false);
    // "packed" with an empty statement carries no proof of anything.
    let r = post(
        &app,
        "/auth/complete",
        registration_json(&ch, &cd, &d.attestation_object("packed")),
    )
    .await;
    assert_ne!(r.0, StatusCode::OK, "{}", r.1);
    assert!(r.1.get("token").is_none());
    assert_eq!(w.keys.len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn tampers_the_client_data_json_after_it_was_signed() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    // Each variant gets a fresh challenge, so the refusal is the signature's
    // and not a spent challenge's.
    for variant in 0..3 {
        let ch = begin_sign_in(&app).await;
        let CompleteRequest::PasskeyAssertion {
            credential_id,
            mut client_data_json,
            authenticator_data,
            signature,
            ..
        } = d.assert(&ch)
        else {
            panic!("assertion")
        };
        match variant {
            0 => client_data_json.insert(1, b' '),
            1 => {
                let last = client_data_json.len() - 2;
                client_data_json[last] ^= 0x01;
            }
            _ => {
                client_data_json.pop();
                client_data_json.extend_from_slice(br#","x":1}"#);
            }
        }
        refused(
            &post(
                &app,
                "/auth/complete",
                assertion_json(
                    &ch,
                    &credential_id,
                    &client_data_json,
                    &authenticator_data,
                    &signature,
                ),
            )
            .await,
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn tampers_the_authenticator_data_after_it_was_signed() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    let ch = begin_sign_in(&app).await;
    let CompleteRequest::PasskeyAssertion {
        credential_id,
        client_data_json,
        mut authenticator_data,
        signature,
        ..
    } = d.assert(&ch)
    else {
        panic!("assertion")
    };
    // Raise the counter after signing.
    authenticator_data[36] = 9;
    refused(
        &post(
            &app,
            "/auth/complete",
            assertion_json(
                &ch,
                &credential_id,
                &client_data_json,
                &authenticator_data,
                &signature,
            ),
        )
        .await,
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sends_garbage_where_the_ceremony_bytes_belong() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    let ch = begin_sign_in(&app).await;
    for (cd, ad, sig) in [
        (&b""[..], &b""[..], &b""[..]),
        (&b"not json"[..], &b"short"[..], &b"x"[..]),
        (&b"{}"[..], &[0u8; 37][..], &[0u8; 64][..]),
    ] {
        let r = post(
            &app,
            "/auth/complete",
            assertion_json(&ch, &d.credential_id, cd, ad, sig),
        )
        .await;
        assert!(r.0.is_client_error(), "{} {}", r.0, r.1);
        assert!(r.1.get("token").is_none());
    }
}

// ═══════════════════════ credentials: whose key, whose id ═════════════════

#[tokio::test(flavor = "multi_thread")]
async fn signs_with_another_credentials_key_under_the_victims_credential_id() {
    let w = world();
    let app = w.app();
    let victim = TestAuthenticator::es256();
    enroll_directly(&w, &victim);
    let other = TestAuthenticator::ed25519();
    enroll_directly(&w, &other);
    let ch = begin_sign_in(&app).await;
    let ad = good_auth_data(&victim);
    refused(
        &post(
            &app,
            "/auth/complete",
            forged(&victim, &other, &ch, "webauthn.get", &victim.origin, &ad),
        )
        .await,
    );
    // Mallory's own, never-enrolled key under the victim's id.
    let mut mallory = TestAuthenticator::es256();
    mallory.key = ordo_identity::software_authenticator::TestKey::es256(99);
    mallory.credential_id = victim.credential_id.clone();
    let ch = begin_sign_in(&app).await;
    refused(&sign_in_with_challenge(&app, &mallory, &ch).await);
}

async fn sign_in_with_challenge(
    app: &axum::Router,
    d: &TestAuthenticator,
    ch: &str,
) -> (StatusCode, Value) {
    post(app, "/auth/complete", assertion_body(&d.assert(ch))).await
}

#[tokio::test(flavor = "multi_thread")]
async fn asserts_for_a_credential_the_person_does_not_hold() {
    let w = world();
    let app = w.app();
    let held = TestAuthenticator::es256();
    let (token, principal) = password_session(&w, &app, &held, 0).await;
    // Another person's passkey, enrolled and signed in elsewhere.
    let theirs = TestAuthenticator::ed25519();
    sign_up(&app, &theirs).await;
    let ch = step_up_challenge(&app, &token).await;
    refused(&step_up(&app, &token, assertion_body(&theirs.assert(&ch))).await);
    // A credential nobody enrolled.
    let mut ghost = TestAuthenticator::es256();
    ghost.credential_id = b"never-enrolled".to_vec();
    let ch = step_up_challenge(&app, &token).await;
    refused(&step_up(&app, &token, assertion_body(&ghost.assert(&ch))).await);
    let door = door_for(&principal, &w, NOW + 10);
    assert_eq!(door.check(&token, SPACE, GATED), AccessDecision::Deny);
    // Sign-in with the ghost gives the same answer as a bad signature.
    let a = sign_in(&app, &ghost).await;
    refused(&a);
    let ch = begin_sign_in(&app).await;
    let mut bad = held.assert(&ch);
    if let CompleteRequest::PasskeyAssertion { signature, .. } = &mut bad {
        signature[0] ^= 1;
    }
    let b = post(&app, "/auth/complete", assertion_body(&bad)).await;
    assert_eq!(a.1, b.1, "unknown credential and bad signature read alike");
}

#[tokio::test(flavor = "multi_thread")]
async fn runs_a_sign_count_backwards_as_a_cloned_authenticator() {
    let w = world();
    let app = w.app();
    let mut d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    d.sign_count = 10;
    assert_eq!(sign_in(&app, &d).await.0, StatusCode::OK);
    // The clone presents a lower count, then the same count.
    d.sign_count = 4;
    refused(&sign_in(&app, &d).await);
    d.sign_count = 10;
    refused(&sign_in(&app, &d).await);
    // The real one carries on.
    d.sign_count = 11;
    assert_eq!(sign_in(&app, &d).await.0, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn runs_a_sign_count_backwards_at_step_up() {
    let w = world();
    let app = w.app();
    let mut d = TestAuthenticator::es256();
    let (token, _) = password_session(&w, &app, &d, 0).await;
    d.sign_count = 20;
    let ch = step_up_challenge(&app, &token).await;
    assert_eq!(
        step_up(&app, &token, assertion_body(&d.assert(&ch)))
            .await
            .0,
        StatusCode::OK
    );
    d.sign_count = 3;
    let ch = step_up_challenge(&app, &token).await;
    refused(&step_up(&app, &token, assertion_body(&d.assert(&ch))).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn registers_another_persons_credential_id_under_a_new_key() {
    let w = world();
    let app = w.app();
    let victim = TestAuthenticator::es256();
    let victim_token = sign_up(&app, &victim).await;
    let mut mallory = TestAuthenticator::es256();
    mallory.key = ordo_identity::software_authenticator::TestKey::es256(99);
    let ch = begin_register(&app).await;
    refused(
        &post(
            &app,
            "/auth/complete",
            registration_body(&mallory.register(&ch)),
        )
        .await,
    );
    assert_eq!(w.keys.len(), 1, "the victim's row is untouched");
    // Mallory's key does not sign in; the victim's still does, as the same person.
    refused(&sign_in(&app, &mallory).await);
    let (status, done) = sign_in(&app, &victim).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(
        principal_of(done["token"].as_str().expect("t")),
        principal_of(&victim_token)
    );
}

// ═══════════════════════ step-up: reuse, window, quarantine ═══════════════

#[tokio::test(flavor = "multi_thread")]
async fn replays_a_step_up_assertion_for_a_second_token() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    let (token, principal) = password_session(&w, &app, &d, 0).await;
    let ch = step_up_challenge(&app, &token).await;
    let body = assertion_body(&d.assert(&ch));
    let (status, done) = step_up(&app, &token, body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    // The same bytes, again, on the same session and on a second login.
    refused(&step_up(&app, &token, body.clone()).await);
    let (second, _) = {
        let (_, begun) = post(
            &app,
            "/auth/begin",
            json!({"method": "password", "identifier": "ana@example.test"}),
        )
        .await;
        let (s, done) = post(&app, "/auth/complete", json!({"method": "password", "kind": "password",
            "challenge": begun["challenge"], "identifier": "ana@example.test", "password": "correct-horse"})).await;
        assert_eq!(s, StatusCode::OK, "{done}");
        (done["token"].as_str().expect("t").to_string(), ())
    };
    refused(&step_up(&app, &second, body).await);
    // The old token gained nothing, and neither did the second.
    let door = door_for(&principal, &w, NOW + 10);
    assert_eq!(door.check(&token, SPACE, GATED), AccessDecision::Deny);
    assert_eq!(door.check(&second, SPACE, GATED), AccessDecision::Deny);
}

#[tokio::test(flavor = "multi_thread")]
async fn uses_a_stepped_up_token_after_the_freshness_window() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    let (token, principal) = password_session(&w, &app, &d, 0).await;
    let ch = step_up_challenge(&app, &token).await;
    let (status, done) = step_up(&app, &token, assertion_body(&d.assert(&ch))).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let stepped = done["token"].as_str().expect("t").to_string();
    assert!(matches!(
        door_for(&principal, &w, NOW + 60).check(&stepped, SPACE, GATED),
        AccessDecision::Allow { .. }
    ));
    assert_eq!(
        door_for(&principal, &w, NOW + 301).check(&stepped, SPACE, GATED),
        AccessDecision::Deny
    );
    assert_eq!(
        door_for(&principal, &w, NOW + 3_000).check(&stepped, SPACE, GATED),
        AccessDecision::Deny
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn uses_a_stepped_up_token_after_logout() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    let (token, principal) = password_session(&w, &app, &d, 0).await;
    let ch = step_up_challenge(&app, &token).await;
    let (_, done) = step_up(&app, &token, assertion_body(&d.assert(&ch))).await;
    let stepped = done["token"].as_str().expect("t").to_string();
    let (status, _) = send(&app, "/auth/logout", None, Some(&stepped)).await;
    assert!(status.is_success(), "{status}");
    assert_eq!(
        door_for(&principal, &w, NOW + 10).check(&stepped, SPACE, GATED),
        AccessDecision::Deny
    );
    assert_eq!(
        door_for(&principal, &w, NOW + 10).check(&token, SPACE, GATED),
        AccessDecision::Deny
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn presents_a_quarantined_passkey_at_a_presence_gated_act() {
    let w = world();
    let app = w.app();
    let d = TestAuthenticator::es256();
    // A recovery added this passkey; its presence counts from NOW + 1 day.
    let (token, principal) = password_session(&w, &app, &d, NOW + 86_400).await;
    let door = door_for(&principal, &w, NOW + 60);
    // Step-up on the password session with the quarantined passkey.
    let ch = step_up_challenge(&app, &token).await;
    let (status, done) = step_up(&app, &token, assertion_body(&d.assert(&ch))).await;
    if status == StatusCode::OK {
        let t = done["token"].as_str().expect("token");
        assert_eq!(
            door.check(t, SPACE, GATED),
            AccessDecision::Deny,
            "no presence earned"
        );
    }
    assert_eq!(door.check(&token, SPACE, GATED), AccessDecision::Deny);
    // Signing in with it gives a session for ordinary acts and none for the gated one.
    let (status, done) = sign_in(&app, &d).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let t = done["token"].as_str().expect("token");
    assert_eq!(principal_of(t), principal);
    assert!(matches!(
        door.check(t, SPACE, PLAIN),
        AccessDecision::Allow { .. }
    ));
    assert_eq!(door.check(t, SPACE, GATED), AccessDecision::Deny);
    // After the quarantine ends the same passkey does count.
    w.set_now(NOW + 86_401);
    let (status, done) = sign_in(&app, &d).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let t = done["token"].as_str().expect("token");
    assert!(matches!(
        door_for(&principal, &w, NOW + 86_402).check(t, SPACE, GATED),
        AccessDecision::Allow { .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn asserts_with_a_preferred_deployment_but_presence_cleared() {
    // A looser deployment (user verification only preferred) still needs presence.
    let rp = RelyingParty::new("example.com", &["https://app.example.com"])
        .expect("rp")
        .with_user_verification(UserVerification::Preferred);
    let w = world_with(rp);
    let app = w.app();
    let mut d = TestAuthenticator::es256();
    enroll_directly(&w, &d);
    d.user_present = false;
    d.user_verified = false;
    refused(&sign_in(&app, &d).await);
    // Presence alone is what this deployment chose to accept, and it does.
    d.user_present = true;
    assert_eq!(sign_in(&app, &d).await.0, StatusCode::OK);
}
