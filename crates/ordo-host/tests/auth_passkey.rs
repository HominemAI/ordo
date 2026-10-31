//! Passkey sign-in at the door: the ceremonies, driven through the real
//! routes by a software authenticator.
//!
//! Nothing here fakes a user gesture (checklist 5h). There is no browser, so
//! `TestAuthenticator` holds a real key and emits the bytes a real
//! authenticator emits, and the routes verify them exactly as they would
//! verify a person's. A person clicks through the first real one.

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
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{
    AccessEngine, AccessProfile, MemorySessionRevocations, PasskeyProvider, PrincipalId,
    ProviderRegistry, RelyingParty, Role, SpaceId, TokenKey,
};
use ordo_secevent::{EventKind, RecordedDecision};
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &[u8] = b"auth-passkey-test-secret";
const NOW: u64 = 1_000;
const SPACE: &str = "spc_clinic";
const GATED: &str = "transfer.approve";
const PLAIN: &str = "txns.list";

pub struct Harness {
    pub auth: Arc<AuthState>,
    pub passkey: Arc<PasskeyProvider>,
    pub keys: Arc<ordo_identity::MemoryPasskeyStore>,
    pub revocations: Arc<MemorySessionRevocations>,
    /// The directory's store, kept so a test can link a second
    /// authenticator to a principal directly, which the ceremony that does
    /// it in production (`link`) needs a presence assertion for.
    pub store: Arc<ordo_identity::MemoryDirectoryStore>,
    /// The deployment's clock, in seconds. A test moves it to play "later".
    pub clock: Arc<std::sync::atomic::AtomicU64>,
}

impl Harness {
    /// Move the deployment's clock to `at`.
    pub fn set_now(&self, at: u64) {
        self.clock.store(at, std::sync::atomic::Ordering::SeqCst);
    }
}

fn relying_party() -> RelyingParty {
    RelyingParty::new("example.com", &["https://app.example.com"]).expect("a relying party")
}

/// A deployment with a password provider (one enrolled person) and a
/// passkey provider over memory stores, a clock pinned to `at`, and a
/// recorder.
pub fn harness_with(provisioning: Provisioning, at: u64) -> Harness {
    let keys = Arc::new(ordo_identity::MemoryPasskeyStore::new());
    let passkey = Arc::new(PasskeyProvider::with_stores(
        relying_party(),
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
        Directory::with_store(provisioning, store.clone()),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations.clone())
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()))
    .with_passkey(passkey.clone());
    let clock = Arc::new(std::sync::atomic::AtomicU64::new(at));
    let read = clock.clone();
    state.now = Box::new(move || read.load(std::sync::atomic::Ordering::SeqCst));
    Harness {
        auth: Arc::new(state),
        passkey,
        keys,
        revocations,
        store,
        clock,
    }
}

pub fn harness() -> Harness {
    harness_with(Provisioning::AutoOnboard, NOW)
}

pub fn app(h: &Harness) -> axum::Router {
    ordo_host::auth::router(Some(h.auth.clone()))
}

pub async fn post(app: &axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    send(app, path, Some(body), None).await
}

pub async fn post_bearer(
    app: &axum::Router,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    send(app, path, body, Some(token)).await
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

/// What a browser would post for an assertion: the four byte strings,
/// base64url without padding, and `client_data_json` exactly as produced.
pub fn assertion_body(req: &CompleteRequest) -> Value {
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
    json!({
        "method": "passkey", "kind": "passkey_assertion", "challenge": challenge.0,
        "credential_id": b64url_encode(credential_id),
        "client_data_json": b64url_encode(client_data_json),
        "authenticator_data": b64url_encode(authenticator_data),
        "signature": b64url_encode(signature),
    })
}

/// Register a device straight through the provider, as test setup: the
/// credential is stored and no principal exists yet.
pub fn enroll_directly(h: &Harness, device: &TestAuthenticator) {
    let now = ordo_identity::Timestamp(NOW);
    let options = h.passkey.begin_registration(now, &[]).expect("options");
    ordo_identity::AuthProvider::complete(&h.passkey, &device.register(&options.challenge), now)
        .expect("registers");
}

/// Sign in with an assertion through the routes and return the token.
pub async fn sign_in_with(app: &axum::Router, device: &TestAuthenticator) -> String {
    let (status, options) = post(app, "/auth/begin", json!({"method": "passkey"})).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let challenge = options["challenge"].as_str().expect("a challenge");
    let (status, done) = post(
        app,
        "/auth/complete",
        with_display(assertion_body(&device.assert(challenge)), "Ana"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done["token"].as_str().expect("a token").to_string()
}

pub fn with_display(mut body: Value, display: &str) -> Value {
    body["display"] = json!(display);
    body
}

/// A door over an engine where `transfer.approve` is step-up gated and
/// `txns.list` is not, with `principal` an owner.
pub fn door_for(principal: &PrincipalId, h: &Harness, at: u64) -> IdentityAccess {
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
    .with_revocations(h.revocations.clone())
}

pub fn principal_of(token: &str, at: u64) -> PrincipalId {
    ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        token,
        ordo_identity::Timestamp(at),
    )
    .expect("a token this deployment minted")
    .principal
}

pub fn failure_reasons(h: &Harness) -> Vec<String> {
    h.auth
        .recorder
        .as_ref()
        .expect("recorder")
        .drain_all(ordo_commandlog::Timestamp(NOW + 10_000))
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::AuthenticationFailed { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

// ══════════════════════════════ S3 · sign in ═════════════════════════════

/// `/auth/begin` for a passkey answers what a browser needs to run
/// `navigator.credentials.get`, and asks for no identifier.
#[tokio::test(flavor = "multi_thread")]
async fn a_passkey_begin_answers_what_a_browser_needs() {
    let h = harness();
    let (status, options) = post(&app(&h), "/auth/begin", json!({"method": "passkey"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(options["method"], "passkey");
    assert_eq!(options["rp_id"], "example.com");
    assert_eq!(options["user_verification"], "required");
    assert_eq!(options["allow_credentials"], json!([]));
    assert_eq!(options["timeout_ms"], 300_000);
    assert_eq!(
        options["challenge"].as_str().expect("challenge").len(),
        43,
        "32 bytes, base64url"
    );
}

/// **A passkey ceremony mints a token that passes step-up**, and a password
/// token, the control, does not.
#[tokio::test(flavor = "multi_thread")]
async fn a_passkey_ceremony_mints_a_token_that_passes_step_up() {
    let h = harness();
    let app = app(&h);
    let device = TestAuthenticator::es256();
    enroll_directly(&h, &device);
    let token = sign_in_with(&app, &device).await;
    let principal = principal_of(&token, NOW);
    let door = door_for(&principal, &h, NOW + 60);

    assert_eq!(
        door.check(&token, SPACE, GATED),
        AccessDecision::Allow {
            principal: principal.0.clone(),
            agent: None,
        },
        "presence from the passkey satisfies the gated op"
    );

    // The control: a password sign-in, the same person shape, the same op.
    let (_, begun) = post(
        &app,
        "/auth/begin",
        json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let (status, done) = post(
        &app,
        "/auth/complete",
        json!({"method": "password", "kind": "password",
               "challenge": begun["challenge"], "identifier": "ana@example.test",
               "password": "correct-horse", "display": "Ana"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let password_token = done["token"].as_str().expect("token").to_string();
    let password_principal = principal_of(&password_token, NOW);
    let door = door_for(&password_principal, &h, NOW + 60);
    let _ = door.take_events();
    assert_eq!(
        door.check(&password_token, SPACE, GATED),
        AccessDecision::Deny
    );
    assert_eq!(
        door.take_events()[0].kind,
        EventKind::denied(RecordedDecision::StepUpRequired),
        "the password token is refused for want of presence, and for nothing else"
    );
    assert!(matches!(
        door.check(&password_token, SPACE, PLAIN),
        AccessDecision::Allow { .. }
    ));
}

/// An unknown credential and a signature that does not verify are the same
/// answer, and the stream says which.
#[tokio::test(flavor = "multi_thread")]
async fn an_assertion_for_an_unknown_credential_is_the_one_answer() {
    let h = harness();
    let app = app(&h);
    let known = TestAuthenticator::es256();
    enroll_directly(&h, &known);
    let stranger = TestAuthenticator::ed25519();

    let mut answers = Vec::new();
    for device in [&stranger, &known] {
        let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
        let ch = options["challenge"]
            .as_str()
            .expect("challenge")
            .to_string();
        let mut body = assertion_body(&device.assert(&ch));
        if device.credential_id == known.credential_id {
            // The known credential, signing with the wrong key: flip a byte.
            let mut sig =
                ordo_identity::webauthn::b64url_decode(body["signature"].as_str().expect("sig"))
                    .expect("decodes");
            let last = sig.len() - 1;
            sig[last] ^= 0xff;
            body["signature"] = json!(b64url_encode(&sig));
        }
        answers.push(post(&app, "/auth/complete", body).await);
    }
    assert_eq!(answers[0].0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        (answers[0].0, answers[0].1.to_string()),
        (answers[1].0, answers[1].1.to_string())
    );
    assert_eq!(
        failure_reasons(&h),
        vec!["bad-credentials".to_string(), "bad-credentials".to_string()],
        "the stream has the reason for both, and both are the same one"
    );
}

/// A captured assertion, posted again, finds no challenge.
#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_assertion_is_refused() {
    let h = harness();
    let app = app(&h);
    let device = TestAuthenticator::es256();
    enroll_directly(&h, &device);

    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let body = assertion_body(&device.assert(ch));
    let (first, _) = post(&app, "/auth/complete", body.clone()).await;
    assert_eq!(first, StatusCode::OK);
    let (again, answer) = post(&app, "/auth/complete", body).await;
    assert_eq!(again, StatusCode::UNAUTHORIZED, "{answer}");
    assert_eq!(failure_reasons(&h), vec!["unknown-challenge".to_string()]);
}

// ═══════════════════════════ S4 · registration ═══════════════════════════

/// What a browser would post for a registration.
pub fn registration_body(req: &CompleteRequest, display: &str) -> Value {
    let CompleteRequest::PasskeyRegistration {
        challenge,
        client_data_json,
        attestation_object,
    } = req
    else {
        panic!("not a registration");
    };
    json!({
        "method": "passkey", "kind": "passkey_registration", "challenge": challenge.0,
        "client_data_json": b64url_encode(client_data_json),
        "attestation_object": b64url_encode(attestation_object),
        "display": display,
    })
}

/// Begin a registration through the route and return its options.
pub async fn begin_registration(app: &axum::Router, display: Option<&str>) -> Value {
    let mut body = json!({"method": "passkey"});
    if let Some(d) = display {
        body["display"] = json!(d);
    }
    let (status, options) = post(app, "/auth/register", body).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    options
}

#[tokio::test(flavor = "multi_thread")]
async fn a_registration_begin_answers_the_creation_options() {
    let h = harness();
    let options = begin_registration(&app(&h), Some("Ana")).await;
    assert_eq!(options["rp_id"], "example.com");
    assert_eq!(options["rp_name"], "example.com");
    assert_eq!(options["algorithms"], json!([-7, -8]));
    assert_eq!(options["resident_key"], "required");
    assert_eq!(options["attestation"], "none");
    assert_eq!(options["user_verification"], "required");
    assert_eq!(options["exclude_credentials"], json!([]));
    assert_eq!(options["user"]["name"], "Ana");
    assert_eq!(options["user"]["display_name"], "Ana");
    assert_eq!(options["user"]["id"].as_str().expect("handle").len(), 43);
    let mut ids = std::collections::BTreeSet::new();
    for _ in 0..3 {
        let o = begin_registration(&app(&h), None).await;
        assert_eq!(o["user"]["name"], "example.com", "the RP name when none");
        ids.insert(o["user"]["id"].as_str().expect("id").to_string());
    }
    assert_eq!(ids.len(), 3, "a fresh handle for every ceremony");
}

/// Under `AutoOnboard`, registering **is** signing up: a principal is minted,
/// the key is stored, and the token carries presence.
#[tokio::test(flavor = "multi_thread")]
async fn a_registration_under_auto_onboard_mints_a_principal_and_a_token_with_presence() {
    let h = harness();
    let app = app(&h);
    let device = TestAuthenticator::es256();
    let options = begin_registration(&app, Some("Ana")).await;
    let challenge = options["challenge"].as_str().expect("challenge");
    let (status, done) = post(
        &app,
        "/auth/complete",
        registration_body(&device.register(challenge), "Ana"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let token = done["token"].as_str().expect("token");
    let principal = principal_of(token, NOW);
    assert!(principal.0.starts_with("usr_"), "{principal}");

    let door = door_for(&principal, &h, NOW + 60);
    assert!(
        matches!(
            door.check(token, SPACE, GATED),
            AccessDecision::Allow { .. }
        ),
        "the registration's own presence satisfies step-up"
    );
    let stored = h
        .passkey
        .stored(&device.credential_id)
        .expect("readable")
        .expect("stored");
    assert_eq!(
        b64url_encode(&stored.user_handle),
        options["user"]["id"].as_str().expect("handle"),
        "the handle the browser was told is the handle the row keeps"
    );
    // The same passkey then signs in to the same person.
    let again = sign_in_with(&app, &device).await;
    assert_eq!(principal_of(&again, NOW), principal);
}

/// Under `InviteOnly` the registration is refused as any bad signature is,
/// and **the key is not stored**.
#[tokio::test(flavor = "multi_thread")]
async fn a_registration_under_invite_only_is_the_one_answer_and_stores_nothing() {
    let h = harness_with(Provisioning::InviteOnly, NOW);
    let app = app(&h);
    let device = TestAuthenticator::es256();
    let options = begin_registration(&app, None).await;
    let challenge = options["challenge"].as_str().expect("challenge");
    let refused = post(
        &app,
        "/auth/complete",
        registration_body(&device.register(challenge), "Stranger"),
    )
    .await;
    // The comparison: a bad signature, from a fresh ceremony.
    let (_, opts) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let mut bad = assertion_body(&device.assert(opts["challenge"].as_str().expect("c")));
    bad["signature"] = json!(b64url_encode(&[0u8; 64]));
    let bad = post(&app, "/auth/complete", bad).await;

    assert_eq!(refused.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        (refused.0, refused.1.to_string()),
        (bad.0, bad.1.to_string())
    );
    assert_eq!(h.keys.len(), 0, "no public key was left behind");
    assert_eq!(
        failure_reasons(&h),
        vec!["not-invited".to_string(), "bad-credentials".to_string()]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn registering_the_same_credential_twice_is_refused() {
    let h = harness();
    let app = app(&h);
    let device = TestAuthenticator::es256();
    let first = {
        let o = begin_registration(&app, None).await;
        let c = o["challenge"].as_str().expect("c");
        post(
            &app,
            "/auth/complete",
            registration_body(&device.register(c), "Ana"),
        )
        .await
    };
    assert_eq!(first.0, StatusCode::OK);
    let second = {
        let o = begin_registration(&app, None).await;
        let c = o["challenge"].as_str().expect("c");
        post(
            &app,
            "/auth/complete",
            registration_body(&device.register(c), "Mallory"),
        )
        .await
    };
    assert_eq!(second.0, StatusCode::UNAUTHORIZED, "{}", second.1);
    assert_eq!(h.keys.len(), 1);
    assert_eq!(
        failure_reasons(&h),
        vec!["credential-already-enrolled".to_string()],
        "the caller gets the one answer and the stream the specific reason"
    );
    // The original key still signs in to the original person.
    let token = sign_in_with(&app, &device).await;
    assert_eq!(
        principal_of(&token, NOW),
        principal_of(first.1["token"].as_str().expect("t"), NOW)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_registration_challenge_cannot_complete_a_sign_in() {
    let h = harness();
    let app = app(&h);
    let device = TestAuthenticator::es256();
    let options = begin_registration(&app, None).await;
    let challenge = options["challenge"].as_str().expect("challenge");
    let crossed = post(
        &app,
        "/auth/complete",
        assertion_body(&device.assert(challenge)),
    )
    .await;
    assert_eq!(crossed.0, StatusCode::UNAUTHORIZED);
    assert_eq!(failure_reasons(&h), vec!["unknown-challenge".to_string()]);
    // The registration is still open: a stranger's crossed attempt did not
    // spend it.
    let (status, _) = post(
        &app,
        "/auth/complete",
        registration_body(&device.register(challenge), "Ana"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// A passkey route with no passkey method configured is the one answer.
#[tokio::test(flavor = "multi_thread")]
async fn register_without_a_passkey_method_is_refused() {
    let revocations = Arc::new(MemorySessionRevocations::new());
    let mut registry = ProviderRegistry::new();
    registry
        .register(
            "password",
            Box::new(LocalPasswordProvider::new(Argon2idHasher::weak_for_tests())),
        )
        .expect("password");
    let state = AuthState::new(
        registry,
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations);
    let app = ordo_host::auth::router(Some(Arc::new(state)));
    let (status, _) = post(&app, "/auth/register", json!({"method": "passkey"})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ═══════════════════════════ S5 · step-up ════════════════════════════════

use ordo_identity::{DirectoryStore, PrincipalKind};

/// A person who holds a password and a passkey, and has signed in with the
/// password: a session with no presence. Returns the session's token.
async fn password_session_with_passkey(
    h: &Harness,
    app: &axum::Router,
    device: &TestAuthenticator,
) -> (String, PrincipalId) {
    let principal = h
        .store
        .create_principal(PrincipalKind::Human, "Ana", ordo_identity::Timestamp(NOW))
        .expect("minted");
    let p = h
        .store
        .principal(&principal)
        .expect("read")
        .expect("exists");
    h.store
        .insert_link(
            &ordo_identity::AuthenticatorId("password:ana@example.test".into()),
            &p,
            ordo_identity::Timestamp(NOW),
        )
        .expect("linked");
    h.store
        .insert_link(
            &device.authenticator_id(),
            &p,
            ordo_identity::Timestamp(NOW),
        )
        .expect("linked");
    enroll_directly(h, device);

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
    let token = done["token"].as_str().expect("token").to_string();
    assert_eq!(principal_of(&token, NOW), principal);
    (token, principal)
}

async fn step_up_options(app: &axum::Router, token: &str) -> (StatusCode, Value) {
    post_bearer(app, "/auth/step-up/begin", token, None).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_password_session_steps_up_with_a_passkey_linked_to_the_same_principal() {
    let h = harness();
    let app = app(&h);
    let device = TestAuthenticator::es256();
    let (token, principal) = password_session_with_passkey(&h, &app, &device).await;
    let door = door_for(&principal, &h, NOW + 60);
    let _ = door.take_events();
    assert_eq!(door.check(&token, SPACE, GATED), AccessDecision::Deny);

    let (status, options) = step_up_options(&app, &token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(
        options["allow_credentials"],
        json!([b64url_encode(&device.credential_id)]),
        "narrowed to this principal's passkeys"
    );
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, done) = post_bearer(
        &app,
        "/auth/step-up",
        &token,
        Some(assertion_body(&device.assert(ch))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let stepped = done["token"].as_str().expect("token");
    assert_ne!(stepped, token);
    assert!(matches!(
        door.check(stepped, SPACE, GATED),
        AccessDecision::Allow { .. }
    ));
    // The same session, and nothing wider.
    let before = ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        &token,
        ordo_identity::Timestamp(NOW),
    )
    .expect("claims");
    let after = ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        stepped,
        ordo_identity::Timestamp(NOW),
    )
    .expect("claims");
    assert_eq!(after.principal, before.principal);
    assert_eq!(after.session, before.session);
    assert_eq!(after.client, before.client);
    assert!(after.user_presence_at.is_some() && before.user_presence_at.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_passkey_of_another_principal_cannot_step_up_a_session() {
    let h = harness();
    let app = app(&h);
    let mine = TestAuthenticator::es256();
    let (token, _) = password_session_with_passkey(&h, &app, &mine).await;

    // Somebody else, with a passkey of their own, on their own session.
    let theirs = TestAuthenticator::ed25519();
    enroll_directly(&h, &theirs);
    let other_token = sign_in_with(&app, &theirs).await;

    // Their passkey answers a challenge minted for my session.
    let (_, options) = step_up_options(&app, &token).await;
    let ch = options["challenge"].as_str().expect("c").to_string();
    let (status, _) = post_bearer(
        &app,
        "/auth/step-up",
        &token,
        Some(assertion_body(&theirs.assert(&ch))),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A challenge minted for their session, answered on mine with my passkey.
    let (_, options) = step_up_options(&app, &other_token).await;
    let ch = options["challenge"].as_str().expect("c").to_string();
    let (status, _) = post_bearer(
        &app,
        "/auth/step-up",
        &token,
        Some(assertion_body(&mine.assert(&ch))),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        failure_reasons(&h),
        vec![
            "passkey-of-another-principal".to_string(),
            "challenge-for-another-principal".to_string()
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_logged_out_session_cannot_step_up() {
    let h = harness();
    let app = app(&h);
    let device = TestAuthenticator::es256();
    let (token, _) = password_session_with_passkey(&h, &app, &device).await;
    let (_, options) = step_up_options(&app, &token).await;
    let ch = options["challenge"].as_str().expect("c").to_string();

    let (status, _) = post_bearer(&app, "/auth/logout", &token, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = step_up_options(&app, &token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no new challenge");
    let (status, _) = post_bearer(
        &app,
        "/auth/step-up",
        &token,
        Some(assertion_body(&device.assert(&ch))),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "and an old one is no use");
    assert_eq!(
        failure_reasons(&h),
        vec!["session-ended".to_string(), "session-ended".to_string()]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_capability_token_cannot_step_up() {
    let h = harness();
    let app = app(&h);
    let device = TestAuthenticator::es256();
    let (_, principal) = password_session_with_passkey(&h, &app, &device).await;

    // The pins are not the point here; the token's shape is.
    let grant = ordo_identity::CapabilityGrant::new(
        ordo_identity::GrantId("gr_1".into()),
        SpaceId(SPACE.into()),
        std::collections::BTreeMap::new(),
        ordo_identity::Timestamp(NOW + 3_600),
    );
    let capability = ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &ordo_identity::TokenSpec {
            principal,
            expires: ordo_identity::Timestamp(NOW + 3_600),
            client: None,
            user_presence_at: None,
            capability: Some(grant),
            delegation: None,
            session: None,
        },
    );
    let (status, _) = step_up_options(&app, &capability).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        failure_reasons(&h),
        vec!["presence-not-delegable".to_string()]
    );
}

/// A step-up challenge is its own phase: it cannot sign anybody in, and a
/// sign-in challenge cannot step a session up.
#[tokio::test(flavor = "multi_thread")]
async fn a_step_up_challenge_and_a_sign_in_challenge_are_not_interchangeable() {
    let h = harness();
    let app = app(&h);
    let device = TestAuthenticator::es256();
    let (token, _) = password_session_with_passkey(&h, &app, &device).await;

    let (_, options) = step_up_options(&app, &token).await;
    let step = options["challenge"].as_str().expect("c").to_string();
    let (status, _) = post(
        &app,
        "/auth/complete",
        assertion_body(&device.assert(&step)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a step-up cannot sign in");

    let (_, begun) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let sign_in = begun["challenge"].as_str().expect("c").to_string();
    let (status, _) = post_bearer(
        &app,
        "/auth/step-up",
        &token,
        Some(assertion_body(&device.assert(&sign_in))),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a sign-in cannot step up");
    // Neither refusal spent the other's challenge.
    let (status, _) = post_bearer(
        &app,
        "/auth/step-up",
        &token,
        Some(assertion_body(&device.assert(&step))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ═══════════════════ S7 · registration by ticket ═════════════════════════

use ordo_host::tickets::{MemoryTicketStore, Purpose, TicketSpec, Tickets};

/// Give the deployment tickets, on a clock pinned to `NOW`.
fn with_tickets(h: &Harness) -> Arc<Tickets> {
    let tickets =
        Arc::new(Tickets::new(Arc::new(MemoryTicketStore::new())).with_clock(Box::new(|| NOW)));
    h.auth.wire_enrolment(ordo_host::auth::Enrolment {
        tickets: Some(tickets.clone()),
        ..Default::default()
    });
    tickets
}

fn issue(tickets: &Tickets, purpose: Purpose, principal: Option<&PrincipalId>) -> String {
    tickets
        .issue(TicketSpec {
            purpose,
            principal: principal.cloned(),
            display: "Ana Sanchez".into(),
            space: None,
            role: None,
            issued_by: "usr_admin".into(),
            ttl_secs: 0,
        })
        .expect("issued")
        .ticket
}

/// A person an administrator created: a principal with no factor yet.
fn created_person(h: &Harness) -> PrincipalId {
    h.store
        .create_principal(PrincipalKind::Human, "Ana", ordo_identity::Timestamp(NOW))
        .expect("minted")
}

async fn register_with(
    app: &axum::Router,
    device: &TestAuthenticator,
    ticket: Option<&str>,
) -> (StatusCode, Value) {
    let mut start = json!({"method": "passkey"});
    if let Some(t) = ticket {
        start["ticket"] = json!(t);
    }
    let (status, options) = post(app, "/auth/register", start).await;
    if status != StatusCode::OK {
        return (status, options);
    }
    let challenge = options["challenge"].as_str().expect("challenge");
    let mut body = registration_body(&device.register(challenge), "Ana");
    if let Some(t) = ticket {
        body["ticket"] = json!(t);
    }
    post(app, "/auth/complete", body).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_registers_a_passkey_for_the_person_it_names() {
    // Under InviteOnly, where a stranger's registration is refused: the
    // ticket is the invitation.
    let h = harness_with(Provisioning::InviteOnly, NOW);
    let tickets = with_tickets(&h);
    let app = app(&h);
    let person = created_person(&h);
    let ticket = issue(&tickets, Purpose::Passkey, Some(&person));
    let device = TestAuthenticator::es256();

    let (status, done) = register_with(&app, &device, Some(&ticket)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let token = done["token"].as_str().expect("token");
    assert_eq!(
        principal_of(token, NOW),
        person,
        "the person the ticket names"
    );
    let door = door_for(&person, &h, NOW + 60);
    assert!(matches!(
        door.check(token, SPACE, GATED),
        AccessDecision::Allow { .. }
    ));
    assert_eq!(
        h.store.authenticators_of(&person).expect("listed"),
        [device.authenticator_id()],
        "the passkey is linked to them"
    );
    assert_eq!(h.keys.len(), 1);
    // The passkey then signs in on its own.
    let again = sign_in_with(&app, &device).await;
    assert_eq!(principal_of(&again, NOW), person);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redeemed_ticket_registers_nothing_a_second_time() {
    let h = harness();
    let tickets = with_tickets(&h);
    let app = app(&h);
    let person = created_person(&h);
    let ticket = issue(&tickets, Purpose::Passkey, Some(&person));

    let first = register_with(&app, &TestAuthenticator::es256(), Some(&ticket)).await;
    assert_eq!(first.0, StatusCode::OK);
    let second = register_with(&app, &TestAuthenticator::ed25519(), Some(&ticket)).await;
    assert_eq!(second.0, StatusCode::UNAUTHORIZED, "{}", second.1);
    assert_eq!(h.keys.len(), 1, "the second device left nothing");
    assert_eq!(h.store.authenticators_of(&person).expect("listed").len(), 1);
}

/// The case for recovery R2's `link`: a person who already holds a factor
/// cannot add one with a ticket. The ticket is not spent by the refusal.
#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_for_a_person_who_already_holds_a_passkey_is_refused() {
    let h = harness();
    let tickets = with_tickets(&h);
    let app = app(&h);
    let person = created_person(&h);
    let held = TestAuthenticator::es256();
    enroll_directly(&h, &held);
    let p = h.store.principal(&person).expect("read").expect("exists");
    h.store
        .insert_link(&held.authenticator_id(), &p, ordo_identity::Timestamp(NOW))
        .expect("linked");

    let ticket = issue(&tickets, Purpose::Passkey, Some(&person));
    let second = TestAuthenticator::ed25519();
    let (status, _) = register_with(&app, &second, Some(&ticket)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.keys.len(), 1);
    assert_eq!(h.store.authenticators_of(&person).expect("listed").len(), 1);
    assert_eq!(
        failure_reasons(&h),
        vec!["principal-holds-a-factor".to_string()]
    );
    assert!(
        tickets
            .peek(
                &ticket,
                &ordo_host::tickets::Want {
                    purpose: Some(Purpose::Passkey),
                    ..Default::default()
                }
            )
            .is_ok(),
        "a refusal does not spend the ticket"
    );
}

/// A ticket that names a deleted account registers nothing: the account is
/// marked and kept (owner ruling 2026-10-06) and never signs in again, and
/// an invitation that names it is not a way back in. The ticket is not
/// spent, the passkey is not stored, and the refusal is the one answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_that_names_a_deleted_account_registers_nothing() {
    let h = harness_with(Provisioning::InviteOnly, NOW);
    let tickets = with_tickets(&h);
    let app = app(&h);
    let person = created_person(&h);
    h.store
        .mark_deleted(&person, ordo_identity::Timestamp(NOW))
        .expect("marked deleted");
    let ticket = issue(&tickets, Purpose::Passkey, Some(&person));

    let (status, done) = register_with(&app, &TestAuthenticator::es256(), Some(&ticket)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{done}");
    assert_eq!(h.keys.len(), 0, "no passkey was stored");
    assert!(
        h.store
            .authenticators_of(&person)
            .expect("listed")
            .is_empty(),
        "nothing was linked to the deleted account"
    );
    assert_eq!(failure_reasons(&h), vec!["identity-deleted".to_string()]);
    assert!(
        tickets
            .peek(
                &ticket,
                &ordo_host::tickets::Want {
                    purpose: Some(Purpose::Passkey),
                    ..Default::default()
                }
            )
            .is_ok(),
        "the refusal does not spend the ticket"
    );
}

/// A ticket for nobody in particular mints the person when it is redeemed.
#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_that_names_nobody_mints_a_person() {
    let h = harness_with(Provisioning::InviteOnly, NOW);
    let tickets = with_tickets(&h);
    let app = app(&h);
    let ticket = issue(&tickets, Purpose::Passkey, None);
    let (status, done) = register_with(&app, &TestAuthenticator::es256(), Some(&ticket)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let principal = principal_of(done["token"].as_str().expect("t"), NOW);
    let minted = h
        .store
        .principal(&principal)
        .expect("read")
        .expect("exists");
    assert_eq!(minted.display, "Ana Sanchez", "the ticket's own display");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_of_another_purpose_is_refused_and_not_spent() {
    let h = harness();
    let tickets = with_tickets(&h);
    let app = app(&h);
    let person = created_person(&h);
    let password_ticket = issue(&tickets, Purpose::Password, Some(&person));
    let (status, _) =
        register_with(&app, &TestAuthenticator::es256(), Some(&password_ticket)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.keys.len(), 0);
    assert!(tickets
        .peek(
            &password_ticket,
            &ordo_host::tickets::Want {
                purpose: Some(Purpose::Password),
                ..Default::default()
            }
        )
        .is_ok());
}

/// A ceremony begun with a ticket cannot finish without it, and one begun
/// without cannot borrow one at the end. Otherwise begin-time and
/// complete-time checks could be split across two tickets.
#[tokio::test(flavor = "multi_thread")]
async fn a_ceremony_is_bound_to_the_ticket_it_began_with() {
    let h = harness_with(Provisioning::InviteOnly, NOW);
    let tickets = with_tickets(&h);
    let app = app(&h);
    let person = created_person(&h);
    let ticket = issue(&tickets, Purpose::Passkey, Some(&person));
    let other = issue(&tickets, Purpose::Passkey, None);
    let device = TestAuthenticator::es256();

    // Begun with a ticket, finished without one.
    let (_, options) = post(
        &app,
        "/auth/register",
        json!({"method": "passkey", "ticket": ticket}),
    )
    .await;
    let c = options["challenge"].as_str().expect("c").to_string();
    let (status, _) = post(
        &app,
        "/auth/complete",
        registration_body(&device.register(&c), "Ana"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Begun with one ticket, finished with another.
    let (_, options) = post(
        &app,
        "/auth/register",
        json!({"method": "passkey", "ticket": ticket}),
    )
    .await;
    let c = options["challenge"].as_str().expect("c").to_string();
    let mut body = registration_body(&device.register(&c), "Ana");
    body["ticket"] = json!(other);
    let (status, _) = post(&app, "/auth/complete", body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Begun with no ticket, finished with one, under a policy that refuses
    // strangers.
    let (_, options) = post(&app, "/auth/register", json!({"method": "passkey"})).await;
    let c = options["challenge"].as_str().expect("c").to_string();
    let mut body = registration_body(&device.register(&c), "Ana");
    body["ticket"] = json!(ticket);
    let (status, _) = post(&app, "/auth/complete", body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    assert_eq!(h.keys.len(), 0);
    assert_eq!(
        failure_reasons(&h),
        vec![
            "ticket-required".to_string(),
            "ticket-binding".to_string(),
            "ticket-binding".to_string()
        ]
    );
    // None of the three spent the ticket.
    let (status, _) = register_with(&app, &device, Some(&ticket)).await;
    assert_eq!(status, StatusCode::OK);
}

/// With no tickets wired, a body that carries one is the one answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_is_refused_where_none_are_honoured() {
    let h = harness();
    let (status, _) = post(
        &app(&h),
        "/auth/register",
        json!({"method": "passkey", "ticket": "tkt_00"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ═══════════════════ R2 · a person's own authenticators ══════════════════

async fn get_bearer(app: &axum::Router, path: &str, token: &str) -> (StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
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

/// A third device, distinct from `es256()` and `ed25519()`.
fn third_device() -> TestAuthenticator {
    let mut d = TestAuthenticator::es256();
    d.credential_id = b"credential-three".to_vec();
    d
}

/// A person who signed up with `device`: the token of that sign-in.
async fn signed_up(app: &axum::Router, device: &TestAuthenticator) -> String {
    let (status, done) = register_with(app, device, None).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done["token"].as_str().expect("token").to_string()
}

/// Add `new` to the person whose session is `token`, vouched by `voucher`.
async fn link_device(
    app: &axum::Router,
    token: &str,
    new: &TestAuthenticator,
    voucher: &TestAuthenticator,
) -> (StatusCode, Value) {
    let (status, opts) = post_bearer(app, "/auth/authenticators/link/begin", token, None).await;
    if status != StatusCode::OK {
        return (status, opts);
    }
    let reg = opts["registration"]["challenge"].as_str().expect("reg");
    let vouch = opts["vouch"]["challenge"].as_str().expect("vouch");
    post_bearer(
        app,
        "/auth/authenticators/link",
        token,
        Some(json!({
            "registration": registration_body(&new.register(reg), "Ana"),
            "vouch": assertion_body(&voucher.assert(vouch)),
        })),
    )
    .await
}

/// Remove `target` from the person whose session is `token`, vouched by
/// `voucher`, through the step-up challenge the session route mints.
async fn unlink_device(
    app: &axum::Router,
    token: &str,
    target: &TestAuthenticator,
    voucher: &TestAuthenticator,
    extra: Value,
) -> (StatusCode, Value) {
    let (status, opts) = step_up_options(app, token).await;
    assert_eq!(status, StatusCode::OK, "{opts}");
    let ch = opts["challenge"].as_str().expect("challenge");
    let mut body = json!({
        "authenticator": target.authenticator_id().0,
        "vouch": assertion_body(&voucher.assert(ch)),
    });
    if let Some(o) = extra.as_object() {
        for (k, v) in o {
            body[k] = v.clone();
        }
    }
    post_bearer(app, "/auth/authenticators/unlink", token, Some(body)).await
}

fn recorded_kinds(h: &Harness) -> Vec<EventKind> {
    h.auth
        .recorder
        .as_ref()
        .expect("recorder")
        .drain_all(ordo_commandlog::Timestamp(NOW + 100_000))
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

/// **A link names both authenticators**: the one that was added and the one
/// that vouched. Read from the security stream, which is where somebody
/// asking "what authorised this factor?" looks.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_is_recorded_naming_both_authenticators() {
    let h = harness();
    let app = app(&h);
    let (a, b) = (TestAuthenticator::es256(), TestAuthenticator::ed25519());
    let token = signed_up(&app, &a).await;
    let principal = principal_of(&token, NOW);
    let _ = recorded_kinds(&h);

    let (status, done) = link_device(&app, &token, &b, &a).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["authenticator"], b.authenticator_id().0);
    assert_eq!(done["authenticators"], 2);

    let linked: Vec<(String, Option<String>)> = recorded_kinds(&h)
        .into_iter()
        .filter_map(|k| match k {
            EventKind::AuthenticatorLinked { added, vouched_by } => Some((added, vouched_by)),
            _ => None,
        })
        .collect();
    assert_eq!(
        linked,
        vec![(b.authenticator_id().0, Some(a.authenticator_id().0))],
        "the stream names what was added and what vouched"
    );

    // The new passkey signs in as the same person, and both are listed.
    let second = sign_in_with(&app, &b).await;
    assert_eq!(principal_of(&second, NOW), principal);
    let (status, list) = get_bearer(&app, "/auth/authenticators", &second).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["authenticators"].as_array().expect("rows").len(), 2);
    assert_eq!(list["notices"], json!([]));
}

/// Security review of console step 3a (finding 2): a vouching touch is
/// bound to the session that asked for it. A relayed touch for one session
/// cannot add a passkey under another session of the same person.
#[tokio::test(flavor = "multi_thread")]
async fn a_vouch_made_for_one_session_does_not_count_on_another() {
    let h = harness();
    let app = app(&h);
    let (a, b) = (TestAuthenticator::es256(), TestAuthenticator::ed25519());
    let mine = signed_up(&app, &a).await;
    let other = sign_in_with(&app, &a).await;
    let (status, opts) = post_bearer(&app, "/auth/authenticators/link/begin", &mine, None).await;
    assert_eq!(status, StatusCode::OK, "{opts}");
    let reg = opts["registration"]["challenge"].as_str().expect("reg");
    let vouch = opts["vouch"]["challenge"].as_str().expect("vouch");
    let (status, body) = post_bearer(
        &app,
        "/auth/authenticators/link",
        &other,
        Some(json!({
            "registration": registration_body(&b.register(reg), "Ana"),
            "vouch": assertion_body(&a.assert(vouch)),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(failure_reasons(&h).contains(&"challenge-for-another-session".to_string()));
    let (_, list) = get_bearer(&app, "/auth/authenticators", &mine).await;
    assert_eq!(list["authenticators"].as_array().expect("rows").len(), 1);
}

/// A link is refused unless an authenticator of **this** person vouches with
/// an assertion made now: a stranger's passkey, a challenge of the wrong
/// kind, and a session that holds no passkey all fail, and nothing is stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_needs_a_vouching_assertion_from_the_persons_own_authenticator() {
    let h = harness();
    let app = app(&h);
    let (a, b, c) = (
        TestAuthenticator::es256(),
        TestAuthenticator::ed25519(),
        third_device(),
    );
    let token = signed_up(&app, &a).await;
    let _stranger = signed_up(&app, &c).await;
    assert_eq!(h.keys.len(), 2);

    // A stranger's passkey vouches on my session.
    let (status, _) = link_device(&app, &token, &b, &c).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The registration half is a sign-up challenge, not the one this route began.
    let (_, opts) = post_bearer(&app, "/auth/authenticators/link/begin", &token, None).await;
    let signup = begin_registration(&app, Some("Ana")).await;
    let (status, _) = post_bearer(
        &app,
        "/auth/authenticators/link",
        &token,
        Some(json!({
            "registration": registration_body(
                &b.register(signup["challenge"].as_str().expect("c")), "Ana"),
            "vouch": assertion_body(&a.assert(opts["vouch"]["challenge"].as_str().expect("v"))),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A person with a password and no passkey has nothing to vouch.
    let (_, begun) = post(
        &app,
        "/auth/begin",
        json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let (_, done) = post(
        &app,
        "/auth/complete",
        json!({"method": "password", "kind": "password",
               "challenge": begun["challenge"], "identifier": "ana@example.test",
               "password": "correct-horse", "display": "Ana"}),
    )
    .await;
    let password_token = done["token"].as_str().expect("token");
    let (status, _) = post_bearer(
        &app,
        "/auth/authenticators/link/begin",
        password_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    assert_eq!(h.keys.len(), 2, "nothing was stored by any of the three");
    let (_, list) = get_bearer(&app, "/auth/authenticators", &token).await;
    assert_eq!(list["authenticators"].as_array().expect("rows").len(), 1);
    let reasons = failure_reasons(&h);
    for want in [
        "passkey-of-another-principal",
        "link-binding",
        "no-passkey-to-vouch",
    ] {
        assert!(
            reasons.iter().any(|r| r == want),
            "missing `{want}` in {reasons:?}"
        );
    }
}

/// **The lost passkey is removed by the one that remains.** The link and the
/// public key both go, every session ends, and the removed credential's
/// assertion is refused while the remaining one signs in afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_passkey_is_removed_by_the_one_that_remains() {
    let h = harness();
    let app = app(&h);
    let (lost, kept) = (TestAuthenticator::es256(), TestAuthenticator::ed25519());
    let stolen_session = signed_up(&app, &lost).await;
    let (status, _) = link_device(&app, &stolen_session, &kept, &lost).await;
    assert_eq!(status, StatusCode::OK);
    let _ = recorded_kinds(&h);

    // The person signs in with the passkey they still hold.
    let session = sign_in_with(&app, &kept).await;
    let (status, done) = unlink_device(&app, &session, &lost, &kept, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["removed"], lost.authenticator_id().0);
    assert_eq!(done["sessions_ended"], true);
    assert_eq!(h.keys.len(), 1, "the public key went with the link");

    // The thief's session, and this one, are ended.
    for t in [&stolen_session, &session] {
        let (status, _) = post_bearer(&app, "/auth/refresh", t, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "an ended session");
    }
    // The lost passkey no longer signs in.
    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, _) = post(&app, "/auth/complete", assertion_body(&lost.assert(ch))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // The remaining one does, after the moment the sessions ended.
    h.set_now(NOW + 5);
    let again = sign_in_with(&app, &kept).await;
    let (status, _) = post_bearer(&app, "/auth/refresh", &again, None).await;
    assert_eq!(status, StatusCode::OK);

    let kinds = recorded_kinds(&h);
    assert!(kinds.contains(&EventKind::AuthenticatorUnlinked {
        removed: lost.authenticator_id().0
    }));
    assert!(kinds.contains(&EventKind::SessionsEnded {
        reason: "authenticator-unlinked".into()
    }));
}

/// The last way in is not removable, whoever asks, and the person is told.
/// Another person's passkey is not removable either.
#[tokio::test(flavor = "multi_thread")]
async fn the_last_passkey_and_another_persons_passkey_are_not_removable() {
    let h = harness();
    let app = app(&h);
    let (a, c) = (TestAuthenticator::es256(), third_device());
    let token = signed_up(&app, &a).await;
    let _other = signed_up(&app, &c).await;

    let (status, body) = unlink_device(&app, &token, &a, &a, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "last_authenticator");

    let (status, _) = unlink_device(&app, &token, &c, &a, json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    assert_eq!(h.keys.len(), 2, "both keys are still stored");
    let (status, _) = post_bearer(&app, "/auth/refresh", &token, None).await;
    assert_eq!(status, StatusCode::OK, "a refused removal ends no session");
}

/// A person with one authenticator is told so, at sign-in and in the list,
/// and is not told once they hold two. Information only.
#[tokio::test(flavor = "multi_thread")]
async fn a_person_with_one_authenticator_is_told_so() {
    let h = harness();
    let app = app(&h);
    let (a, b) = (TestAuthenticator::es256(), TestAuthenticator::ed25519());
    let (status, done) = register_with(&app, &a, None).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["authenticators"], 1);
    assert_eq!(done["notices"], json!(["single_authenticator"]));
    let token = done["token"].as_str().expect("token").to_string();
    let (_, list) = get_bearer(&app, "/auth/authenticators", &token).await;
    assert_eq!(list["notices"], json!(["single_authenticator"]));
    assert_eq!(list["authenticators"][0]["kind"], "passkey");
    assert_eq!(list["authenticators"][0]["backup_eligible"], false);

    let (status, _) = link_device(&app, &token, &b, &a).await;
    assert_eq!(status, StatusCode::OK);
    let again = sign_in_with(&app, &a).await;
    let (_, list) = get_bearer(&app, "/auth/authenticators", &again).await;
    assert_eq!(list["notices"], json!([]));
    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (_, done) = post(&app, "/auth/complete", assertion_body(&a.assert(ch))).await;
    assert_eq!(done["authenticators"], 2);
    assert!(done.get("notices").is_none());
}
