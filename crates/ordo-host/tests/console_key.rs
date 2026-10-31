//! The console key and the relayed touch (`plan-console.md` 8.2 and 8.3,
//! G14 and G15), driven through the real sign-in routes.
//!
//! A console key signs in as the deployment owner with a short session that
//! carries no presence, so it never satisfies step-up. A write that needs a
//! touch gets the owner's own passkey assertion, relayed by the console,
//! against a challenge bound to that one console session.
//!
//! The keys live in memory here; the routes, the ops and the door are the
//! ones a deployment serves.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::{Signer as _, SigningKey};
use ordo_api::{AccessChecker, AccessDecision, BoundParams, ErrorCode};
use ordo_host::auth::AuthState;
use ordo_host::console_keys::{
    ConsoleKeyOps, ConsoleKeyStore, ConsoleKeys, ADD, LIST, REVOKE, SIGNED_PREFIX,
};
use ordo_host::metrics::Metrics;
use ordo_host::IdentityAccess;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity::webauthn::{b64url_decode, b64url_encode};
use ordo_identity::{
    AccessEngine, AccessProfile, CompleteRequest, DirectoryStore, MemorySessionRevocations,
    PasskeyProvider, PrincipalId, PrincipalKind, ProviderRegistry, RelyingParty, Role, SpaceId,
    TokenKey,
};
use ordo_secevent::EventKind;
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &[u8] = b"console-key-test-secret";
const NOW: u64 = 1_000_000;
const HOME: &str = "home";
const GATED: &str = "std.deployment.apply";
const PLAIN: &str = "std.deployment.observedCost";
/// A tenant op a console session may reach with a touch (step 5).
const EXPORT: &str = ordo_host::exit_ops::EXPORT;
/// A tenant the owner also owns.
const OTHER: &str = "tenant-a";
/// A tenant where the deployment owner holds no role.
const NO_ROLE: &str = "tenant-b";
/// A People read (G5), on the tenant list since step 8.
const MEMBERS: &str = ordo_host::people_ops::MEMBERS;
/// Where a hand-off may return to.
const APP: &str = "https://app.example.com";

struct H {
    auth: Arc<AuthState>,
    passkey: Arc<PasskeyProvider>,
    revocations: Arc<MemorySessionRevocations>,
    store: Arc<ordo_identity::MemoryDirectoryStore>,
    ops: ConsoleKeyOps,
    clock: Arc<AtomicU64>,
    owner: PrincipalId,
    device: TestAuthenticator,
    ceremonies: Arc<ordo_identity::MemoryCeremonyStore>,
}

impl H {
    fn at(&self, t: u64) {
        self.clock.store(t, Ordering::SeqCst);
    }
    fn now(&self) -> u64 {
        self.clock.load(Ordering::SeqCst)
    }
    fn app(&self) -> axum::Router {
        ordo_host::auth::router(Some(self.auth.clone()))
    }
    fn reasons(&self) -> Vec<String> {
        self.auth
            .recorder
            .as_ref()
            .expect("recorder")
            .drain_all(ordo_commandlog::Timestamp(NOW + 100_000))
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::AuthenticationFailed { reason } => Some(reason.clone()),
                _ => None,
            })
            .collect()
    }
    fn events(&self) -> Vec<EventKind> {
        self.auth
            .recorder
            .as_ref()
            .expect("recorder")
            .drain_all(ordo_commandlog::Timestamp(NOW + 100_000))
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }

    /// The door: `GATED` needs a touch, `PLAIN` does not, the owner may
    /// call both in the home space and in a tenant, `OTHER`, they also own.
    fn door(&self, at: u64) -> IdentityAccess {
        let mut engine = AccessEngine::new();
        for space in [HOME, OTHER, NO_ROLE] {
            let sid = SpaceId(space.into());
            engine.set_profile(
                &sid,
                AccessProfile::new()
                    .allow(Role::Owner, PLAIN)
                    .allow(Role::Owner, GATED)
                    .allow(Role::Owner, EXPORT)
                    .allow(Role::Owner, MEMBERS),
            );
            // The owner holds a role in every space but `NO_ROLE`.
            if space != NO_ROLE {
                engine.set_membership(&sid, &self.owner, Role::Owner);
            }
        }
        engine.require_step_up(GATED);
        engine.require_step_up(EXPORT);
        IdentityAccess::new(
            engine,
            TokenKey::from_secret(SECRET),
            Box::new(move || at),
            Arc::new(Metrics::new()),
        )
        .with_revocations(self.revocations.clone())
        .with_console_home(HOME)
    }

    async fn op(&self, name: &str, params: Value) -> Result<Value, ordo_api::ExecError> {
        let Value::Object(map) = params else {
            panic!("params are an object")
        };
        self.ops
            .execute(name, HOME, &BoundParams(map), &self.owner.0)
            .await
            .map(|r| Value::Array(r.rows))
    }

    /// Enrol a new key pair for the owner; answers it with its id.
    async fn enrol(&self, seed: u8) -> (SigningKey, String) {
        let sk = SigningKey::from_bytes(&[seed; 32]);
        let pk = b64url_encode(sk.verifying_key().as_bytes());
        let rows = self
            .op(
                ADD,
                json!({"publicKey": pk, "name": format!("console {seed}")}),
            )
            .await
            .expect("enrolled");
        let id = rows[0]["id"].as_str().expect("an id").to_string();
        (sk, id)
    }
}

fn harness_with_session(secs: u64) -> H {
    harness_with(secs, ConsoleKeyStore::memory())
}

fn harness_with(secs: u64, key_store: ConsoleKeyStore) -> H {
    harness_full(secs, key_store, None)
}

fn harness_full(secs: u64, key_store: ConsoleKeyStore, closed_by: Option<&'static str>) -> H {
    let rp = RelyingParty::new("example.com", &["https://app.example.com"]).expect("rp");
    let ceremonies = Arc::new(ordo_identity::MemoryCeremonyStore::new());
    let passkey = Arc::new(PasskeyProvider::with_stores(
        rp,
        Arc::new(ordo_identity::MemoryPasskeyStore::new()),
        ceremonies.clone(),
    ));
    let mut registry = ProviderRegistry::new();
    registry
        .register("passkey", Box::new(passkey.clone()))
        .expect("passkey");
    let revocations = Arc::new(MemorySessionRevocations::new());
    let store = Arc::new(ordo_identity::MemoryDirectoryStore::new());
    let recorder = Arc::new(ordo_secevent::Recorder::default());
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::InviteOnly, store.clone()),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations.clone())
    .with_recorder(recorder.clone())
    .with_passkey(passkey.clone())
    .with_ceremonies(ceremonies.clone())
    .with_handoff(
        Some(ordo_host::federation::parse_return_urls(APP, false).expect("returns")),
        60,
    );
    let clock = Arc::new(AtomicU64::new(NOW));
    let read = clock.clone();
    state.now = Box::new(move || read.load(Ordering::SeqCst));
    let mut keys = ConsoleKeys::new(key_store, [5u8; 32], secs, HOME);
    if let Some(setting) = closed_by {
        keys = keys.closed_by(setting);
    }
    let keys = Arc::new(keys);
    state.wire_console_keys(keys.clone());
    let read = clock.clone();
    let ops = ConsoleKeyOps {
        keys,
        revocations: Some(revocations.clone()),
        now: Arc::new(move || read.load(Ordering::SeqCst)),
        stream: Some(recorder),
    };

    // The deployment owner, with one passkey.
    let owner = store
        .create_principal(PrincipalKind::Human, "Ana", ordo_identity::Timestamp(NOW))
        .expect("minted");
    let p = store.principal(&owner).expect("read").expect("exists");
    let device = TestAuthenticator::es256();
    store
        .insert_link(
            &device.authenticator_id(),
            &p,
            ordo_identity::Timestamp(NOW),
        )
        .expect("linked");
    let options = passkey
        .begin_registration(ordo_identity::Timestamp(NOW), &[])
        .expect("options");
    ordo_identity::AuthProvider::complete(
        &passkey,
        &device.register(&options.challenge),
        ordo_identity::Timestamp(NOW),
    )
    .expect("registers");
    H {
        auth: Arc::new(state),
        passkey,
        revocations,
        store,
        ops,
        clock,
        owner,
        device,
        ceremonies,
    }
}

fn harness() -> H {
    harness_with_session(ordo_host::console_keys::DEFAULT_SESSION_SECS)
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
            b.body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
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

fn sign(sk: &SigningKey, challenge: &str) -> String {
    let mut msg = SIGNED_PREFIX.to_vec();
    msg.extend_from_slice(&b64url_decode(challenge).expect("challenge"));
    b64url_encode(&sk.sign(&msg).to_bytes())
}

async fn begin(app: &axum::Router, key: &str) -> String {
    let (status, body) = send(
        app,
        "/auth/begin",
        Some(json!({"method": "console", "key": key})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["method"], "console");
    body["challenge"].as_str().expect("challenge").to_string()
}

async fn complete(
    app: &axum::Router,
    key: &str,
    challenge: &str,
    signature: &str,
) -> (StatusCode, Value) {
    send(
        app,
        "/auth/complete",
        Some(json!({"method": "console", "key": key, "challenge": challenge, "signature": signature})),
        None,
    )
    .await
}

/// Sign in with a key; answers the token.
async fn sign_in(app: &axum::Router, sk: &SigningKey, key: &str) -> String {
    let ch = begin(app, key).await;
    let (status, body) = complete(app, key, &ch, &sign(sk, &ch)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["token"].as_str().expect("token").to_string()
}

fn claims(token: &str, at: u64) -> ordo_identity::TokenClaims {
    ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        token,
        ordo_identity::Timestamp(at),
    )
    .expect("a token this deployment minted")
}

fn allowed(d: AccessDecision) -> bool {
    matches!(d, AccessDecision::Allow { .. })
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
    json!({
        "method": "passkey", "kind": "passkey_assertion", "challenge": challenge.0,
        "credential_id": b64url_encode(credential_id),
        "client_data_json": b64url_encode(client_data_json),
        "authenticator_data": b64url_encode(authenticator_data),
        "signature": b64url_encode(signature),
    })
}

/// The console relays one touch: step-up options for its session, the
/// owner's assertion made in the page, handed back. Answers the status
/// and body of `/auth/step-up`, and the assertion for a replay.
async fn relay_touch(h: &H, app: &axum::Router, token: &str) -> (StatusCode, Value, Value) {
    let (status, options) = send(app, "/auth/step-up/begin", None, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let assertion = assertion_body(&h.device.assert(options["challenge"].as_str().expect("c")));
    let (status, body) = send(app, "/auth/step-up", Some(assertion.clone()), Some(token)).await;
    (status, body, assertion)
}

// ═══════════════════════════ G14 · the console key ════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn an_enrolled_key_signs_in_as_the_owner_for_a_short_session_with_no_presence() {
    let h = harness();
    let app = h.app();
    let (sk, id) = h.enrol(1).await;
    let ch = begin(&app, &id).await;
    let (status, body) = complete(&app, &id, &ch, &sign(&sk, &ch)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["space"], HOME);
    assert_eq!(body["key"], id.as_str());
    assert_eq!(body["expires_at"], NOW + 900, "15 minutes by default");
    assert_eq!(body["session_expires_at"], NOW + 900, "and no further");
    let token = body["token"].as_str().expect("token");
    let c = claims(token, NOW);
    assert_eq!(c.principal, h.owner);
    assert!(
        c.user_presence_at.is_none(),
        "a console key never carries presence"
    );
    assert!(c.capability.is_none() && c.delegation.is_none());
    assert_eq!(
        c.session.as_ref().and_then(|s| s.via.clone()),
        Some(format!("console:{id}"))
    );

    // What the owner may do without a touch, and nothing that needs one.
    let door = h.door(NOW + 1);
    assert!(allowed(door.check(token, HOME, PLAIN)));
    assert_eq!(door.check(token, HOME, GATED), AccessDecision::Deny);
    // The list shows the key's last use.
    let rows = h.op(LIST, json!({})).await.expect("listed");
    assert_eq!(rows[0]["lastUsedAt"], NOW);
    assert!(h
        .events()
        .iter()
        .any(|e| matches!(e, EventKind::ConsoleKey { act, .. } if act == "signed-in")));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_session_ends_on_time_and_is_renewed_only_by_signing_again() {
    let h = harness();
    let app = h.app();
    let (sk, id) = h.enrol(1).await;
    let token = sign_in(&app, &sk, &id).await;
    let (status, _) = send(&app, "/auth/refresh", None, Some(&token)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "no refresh for a console key"
    );
    assert!(ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        &token,
        ordo_identity::Timestamp(NOW + 901)
    )
    .is_err());
    assert_eq!(
        h.door(NOW + 901).check(&token, HOME, PLAIN),
        AccessDecision::Deny
    );
    h.at(NOW + 901);
    let again = sign_in(&app, &sk, &id).await;
    assert!(allowed(h.door(NOW + 902).check(&again, HOME, PLAIN)));
    // Refused at the layer over `/auth/*` before the route's own refusal.
    assert_eq!(h.reasons(), vec![GATE.to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_challenge_is_single_use_and_bound_to_its_key_and_the_signature_to_the_prefix() {
    let h = harness();
    let app = h.app();
    let (sk, id) = h.enrol(1).await;
    let (sk2, id2) = h.enrol(2).await;

    // A challenge for one key, signed by the other key and presented as it.
    let ch = begin(&app, &id).await;
    let (status, _) = complete(&app, &id2, &ch, &sign(&sk2, &ch)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A bare signature, without the prefix.
    let ch = begin(&app, &id).await;
    let bare = b64url_encode(&sk.sign(&b64url_decode(&ch).unwrap()).to_bytes());
    let (status, _) = complete(&app, &id, &ch, &bare).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A failure spends nothing (nothing is written for a signature that did
    // not verify), so the key's own signature still completes it.
    let (status, _) = complete(&app, &id, &ch, &sign(&sk, &ch)).await;
    assert_eq!(status, StatusCode::OK);

    // A challenge altered in one byte is no challenge of this deployment's.
    let ch = begin(&app, &id).await;
    let mut bytes = b64url_decode(&ch).unwrap();
    bytes[0] ^= 1;
    let forged = b64url_encode(&bytes);
    let (status, _) = complete(&app, &id, &forged, &sign(&sk, &forged)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A good one, replayed.
    let ch = begin(&app, &id).await;
    let sig = sign(&sk, &ch);
    assert_eq!(complete(&app, &id, &ch, &sig).await.0, StatusCode::OK);
    assert_eq!(
        complete(&app, &id, &ch, &sig).await.0,
        StatusCode::UNAUTHORIZED
    );

    // An expired one.
    let ch = begin(&app, &id).await;
    h.at(NOW + 61);
    assert_eq!(
        complete(&app, &id, &ch, &sign(&sk, &ch)).await.0,
        StatusCode::UNAUTHORIZED
    );

    // An unknown key gets a challenge all the same, and the one answer.
    let ghost = "ck-00000000000000000000000000000000";
    let ch = begin(&app, ghost).await;
    assert_eq!(
        complete(&app, ghost, &ch, &sign(&sk, &ch)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.reasons(),
        vec![
            "challenge-for-another-key",
            "bad-signature",
            "challenge-for-another-key",
            "challenge-spent",
            "challenge-expired",
            "unknown-console-key",
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_key_cannot_sign_in_and_its_open_sessions_end_at_once() {
    let h = harness();
    let app = h.app();
    let (sk, id) = h.enrol(1).await;
    let token = sign_in(&app, &sk, &id).await;
    assert!(allowed(h.door(NOW + 1).check(&token, HOME, PLAIN)));

    let rows = h.op(REVOKE, json!({"id": id})).await.expect("revoked");
    assert_eq!(rows[0]["sessionsEnded"], 1);
    assert_eq!(rows[0]["revokedBy"], h.owner.0.as_str());
    // G12: the add and the revoke each left a record in the deployment's
    // stream, naming the key (console step 9's guard reads this test).
    let acts: Vec<String> = h
        .events()
        .iter()
        .filter_map(|e| match e {
            EventKind::ConsoleKey { act, .. } => Some(act.clone()),
            _ => None,
        })
        .collect();
    assert!(acts.iter().any(|a| a == "added"), "{acts:?}");
    assert!(acts.iter().any(|a| a == "revoked"), "{acts:?}");
    assert_eq!(
        h.door(NOW + 1).check(&token, HOME, PLAIN),
        AccessDecision::Deny
    );
    let ch = begin(&app, &id).await;
    assert_eq!(
        complete(&app, &id, &ch, &sign(&sk, &ch)).await.0,
        StatusCode::UNAUTHORIZED
    );
    // A second revocation names the op that lists the keys.
    let e = h.op(REVOKE, json!({"id": id})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    assert!(e.message.contains(LIST), "{}", e.message);
    assert!(h.reasons().contains(&"console-key-revoked".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rotation_enrols_the_new_key_retires_the_old_and_holds_at_most_two() {
    let h = harness();
    let app = h.app();
    let (old_sk, old) = h.enrol(1).await;
    let (new_sk, new) = h.enrol(2).await;
    // A third live key is refused, naming what to do.
    let sk3 = SigningKey::from_bytes(&[3; 32]);
    let e = h
        .op(
            ADD,
            json!({"publicKey": b64url_encode(sk3.verifying_key().as_bytes()), "name": "three"}),
        )
        .await
        .unwrap_err();
    assert!(e.message.contains(REVOKE), "{}", e.message);
    // The same public key twice is refused too.
    h.op(REVOKE, json!({"id": old})).await.expect("old retired");
    let e = h
        .op(
            ADD,
            json!({"publicKey": b64url_encode(new_sk.verifying_key().as_bytes()), "name": "again"}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::TenantRefused);

    let token = sign_in(&app, &new_sk, &new).await;
    assert!(allowed(h.door(NOW + 1).check(&token, HOME, PLAIN)));
    let ch = begin(&app, &old).await;
    assert_eq!(
        complete(&app, &old, &ch, &sign(&old_sk, &ch)).await.0,
        StatusCode::UNAUTHORIZED
    );

    // A malformed public key or name says what a good one is.
    let e = h
        .op(ADD, json!({"publicKey": "abc", "name": "x"}))
        .await
        .unwrap_err();
    assert!(e.message.contains("Ed25519"), "{}", e.message);
    // And the ops answer only in the deployment's own space.
    let e = h
        .ops
        .execute(
            LIST,
            "elsewhere",
            &BoundParams(Default::default()),
            &h.owner.0,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
}

// ════════════════════ G15 · the owner's touch, relayed ════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn a_relayed_touch_makes_the_console_session_present_once_and_no_longer() {
    let h = harness();
    let app = h.app();
    let (sk, id) = h.enrol(1).await;
    let token = sign_in(&app, &sk, &id).await;
    assert_eq!(
        h.door(NOW + 1).check(&token, HOME, GATED),
        AccessDecision::Deny
    );

    h.at(NOW + 100);
    let (status, body, assertion) = relay_touch(&h, &app, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let present = body["token"].as_str().expect("token");
    assert!(allowed(h.door(NOW + 101).check(present, HOME, GATED)));
    // The same session, and the console session's own end: a touch does
    // not lengthen it.
    let (before, after) = (claims(&token, NOW), claims(present, NOW + 100));
    assert_eq!(before.session, after.session);
    assert_eq!(body["session_expires_at"], NOW + 900);
    assert_eq!(body["expires_at"], NOW + 900);
    // Present for 300 seconds, then not.
    assert_eq!(
        h.door(NOW + 401).check(present, HOME, GATED),
        AccessDecision::Deny
    );

    // The same assertion, replayed: the challenge is spent.
    let (status, _) = send(&app, "/auth/step-up", Some(assertion), Some(&token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(h.reasons().iter().any(|r| r.contains("challenge")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_touch_is_bound_to_the_session_it_was_asked_for_and_expires() {
    let h = harness();
    let app = h.app();
    let (sk, id) = h.enrol(1).await;
    let mine = sign_in(&app, &sk, &id).await;
    let other = sign_in(&app, &sk, &id).await;

    // A challenge minted for one console session, answered on another of
    // the same owner.
    let (_, options) = send(&app, "/auth/step-up/begin", None, Some(&mine)).await;
    let assertion = assertion_body(&h.device.assert(options["challenge"].as_str().unwrap()));
    let (status, _) = send(&app, "/auth/step-up", Some(assertion), Some(&other)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A challenge answered after its 300 seconds.
    let (_, options) = send(&app, "/auth/step-up/begin", None, Some(&mine)).await;
    h.at(NOW + 301);
    let assertion = assertion_body(&h.device.assert(options["challenge"].as_str().unwrap()));
    let (status, _) = send(&app, "/auth/step-up", Some(assertion), Some(&mine)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A revoked key's session cannot be stepped up at all.
    h.op(REVOKE, json!({"id": id})).await.expect("revoked");
    let (status, _) = send(&app, "/auth/step-up/begin", None, Some(&mine)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let reasons = h.reasons();
    assert_eq!(reasons[0], "challenge-for-another-session");
    assert!(
        reasons.contains(&"session-ended".to_string()),
        "{reasons:?}"
    );
    let _ = (&h.passkey, &h.store);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_touch_by_a_passkey_of_somebody_else_does_not_count() {
    let h = harness();
    let app = h.app();
    let (sk, id) = h.enrol(1).await;
    let token = sign_in(&app, &sk, &id).await;
    // Somebody else's passkey, enrolled at this deployment.
    let stranger = TestAuthenticator::ed25519();
    let someone = h
        .store
        .create_principal(PrincipalKind::Human, "Eve", ordo_identity::Timestamp(NOW))
        .unwrap();
    let p = h.store.principal(&someone).unwrap().unwrap();
    h.store
        .insert_link(
            &stranger.authenticator_id(),
            &p,
            ordo_identity::Timestamp(NOW),
        )
        .unwrap();
    let options = h
        .passkey
        .begin_registration(ordo_identity::Timestamp(NOW), &[])
        .unwrap();
    ordo_identity::AuthProvider::complete(
        &h.passkey,
        &stranger.register(&options.challenge),
        ordo_identity::Timestamp(NOW),
    )
    .unwrap();
    let (_, options) = send(&app, "/auth/step-up/begin", None, Some(&token)).await;
    let assertion = assertion_body(&stranger.assert(options["challenge"].as_str().unwrap()));
    let (status, _) = send(&app, "/auth/step-up", Some(assertion), Some(&token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let _ = h.now();
}

// ═════════ the door's own caller: a member id, not the principal ═════════

struct NoGrants;

#[async_trait::async_trait]
impl ordo_identity::grants::GrantRegistry for NoGrants {
    async fn record(
        &self,
        _: &ordo_identity::grants::NewGrant,
    ) -> Result<(), ordo_identity::grants::GrantError> {
        Ok(())
    }
    async fn revoke(
        &self,
        _: &SpaceId,
        _: &ordo_identity::GrantId,
        _: ordo_identity::Timestamp,
    ) -> Result<(), ordo_identity::grants::GrantError> {
        Ok(())
    }
    async fn callers_of(
        &self,
        _: &SpaceId,
        _: &str,
        _: ordo_identity::Timestamp,
    ) -> Result<Vec<ordo_identity::grants::GrantRecord>, ordo_identity::grants::GrantError> {
        Ok(Vec::new())
    }
    async fn grants_of(
        &self,
        _: &SpaceId,
        _: &PrincipalId,
    ) -> Result<Vec<ordo_identity::grants::GrantRecord>, ordo_identity::grants::GrantError> {
        Ok(Vec::new())
    }
    async fn is_live(
        &self,
        _: &SpaceId,
        _: &ordo_identity::GrantId,
        _: ordo_identity::Timestamp,
    ) -> Result<bool, ordo_identity::grants::GrantError> {
        Ok(true)
    }
}

/// A key enrolled through the door is the principal's, not the member id the
/// door hands an op as `$caller`. Enrolled through `PlatformOps` with the
/// caller the door built, the console session names the owner: an op runs
/// and the relayed touch counts.
#[tokio::test(flavor = "multi_thread")]
async fn a_key_enrolled_through_the_door_signs_in_as_the_principal_not_the_member_id() {
    let h = harness();
    let app = h.app();
    let sid = SpaceId(HOME.into());
    let build = || {
        let mut engine = AccessEngine::new();
        engine.set_profile(
            &sid,
            AccessProfile::new()
                .allow(Role::Owner, PLAIN)
                .allow(Role::Owner, GATED)
                .allow(Role::Owner, ADD),
        );
        // The member id differs from the principal, as it does in a deployment.
        engine.set_member(&sid, &h.owner, "mem_ana", Some(Role::Owner));
        engine.require_step_up(GATED);
        engine
    };
    let door = IdentityAccess::new(
        build(),
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW + 1),
        Arc::new(Metrics::new()),
    )
    .with_revocations(h.revocations.clone())
    .with_console_home(HOME);
    let read = h.clock.clone();
    let platform = ordo_host::platform_ops::PlatformOps::new(
        Arc::new(build()),
        Arc::new(NoGrants),
        Arc::new(move || read.load(Ordering::SeqCst)),
    )
    .with_console_keys(Arc::new(ConsoleKeyOps {
        keys: h.auth.console_keys().expect("wired").clone(),
        revocations: Some(h.revocations.clone()),
        now: h.ops.now.clone(),
        stream: None,
    }));

    // The owner's own session, as the door sees it.
    let spec = ordo_identity::TokenSpec {
        principal: h.owner.clone(),
        expires: ordo_identity::Timestamp(NOW + 600),
        client: None,
        user_presence_at: Some(ordo_identity::Timestamp(NOW)),
        capability: None,
        delegation: None,
        session: Some(ordo_identity::SessionClaim::starting_at(
            ordo_identity::Timestamp(NOW),
        )),
    };
    let own = ordo_identity::token::mint(&TokenKey::from_secret(SECRET), &spec);
    let AccessDecision::Allow {
        principal: caller, ..
    } = door.check(&own, HOME, ADD)
    else {
        panic!("the owner may enrol a key");
    };
    assert_eq!(caller, "mem_ana", "the door hands the member id as $caller");

    let sk = SigningKey::from_bytes(&[9; 32]);
    let pk = b64url_encode(sk.verifying_key().as_bytes());
    let add = ordo_host::console_keys::specs()
        .into_iter()
        .find(|s| s.name == ADD)
        .expect("the op is defined");
    let Value::Object(params) = json!({"publicKey": pk, "name": "door console"}) else {
        unreachable!()
    };
    let out = platform
        .execute(&add, HOME, &BoundParams(params), &caller)
        .await
        .expect("enrolled through the door");
    let id = out.rows[0]["id"].as_str().expect("an id").to_string();

    let token = sign_in(&app, &sk, &id).await;
    assert_eq!(claims(&token, NOW).principal, h.owner);
    assert!(allowed(door.check(&token, HOME, PLAIN)));
    assert_eq!(door.check(&token, HOME, GATED), AccessDecision::Deny);

    // The relayed touch counts, and the gated op then runs.
    let (status, body, _) = relay_touch(&h, &app, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let present = body["token"].as_str().expect("token");
    assert!(allowed(door.check(present, HOME, GATED)));
}

// ═══════════════ security review of step 3a: where a session may go ═══════

/// Every `"/auth/..."` path mounted in the auth sources, read from the code
/// as `served_routes.rs` does, so a route added later is scanned too.
fn mounted_auth_paths() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![std::fs::read_to_string(dir.join("auth.rs")).unwrap()];
    for e in std::fs::read_dir(dir.join("auth")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            files.push(std::fs::read_to_string(p).unwrap());
        }
    }
    let mut out = Vec::new();
    for f in files {
        let code: String = f
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut rest = code.as_str();
        while let Some(i) = rest.find(".route(") {
            rest = &rest[i + 7..];
            if let Some(t) = rest.trim_start().strip_prefix('"') {
                if let Some(end) = t.find('"') {
                    if t[..end].starts_with("/auth/") {
                        out.push(t[..end].to_string());
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

async fn send_as(app: &axum::Router, method: &str, path: &str, bearer: &str) -> StatusCode {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {bearer}"));
    if method == "POST" {
        b = b.header("content-type", "application/json");
    }
    let body = if method == "POST" {
        Body::from("{}")
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(b.body(body).expect("request"))
        .await
        .expect("response")
        .status()
}

const GATE: &str = "console-session-not-for-this-route";

/// Finding 2, and the guard: a console key session reaches only the relayed
/// touch and its own logout among the `/auth/*` routes. Every other mounted
/// route, including any added later, refuses it at the one layer.
#[tokio::test(flavor = "multi_thread")]
async fn a_console_session_reaches_only_the_touch_and_its_logout_among_the_auth_routes() {
    let h = harness();
    let app = h.app();
    let (sk, id) = h.enrol(1).await;
    let token = sign_in(&app, &sk, &id).await;
    let _ = h.reasons();
    let paths = mounted_auth_paths();
    assert!(paths.len() > 40, "found only {} paths", paths.len());
    for path in &paths {
        let uri = path
            .replace("{connection}", "x")
            .replace("{tenant}", "t")
            .replace("{id}", "x");
        let open = ordo_host::console_keys::AUTH_ROUTES_OPEN.contains(&path.as_str());
        if path == "/auth/logout" {
            continue;
        }
        for method in ["GET", "POST"] {
            let status = send_as(&app, method, &uri, &token).await;
            if !open {
                assert!(!status.is_success(), "{method} {path} answered {status}");
            }
        }
        let reasons = h.reasons();
        assert_eq!(
            reasons.iter().any(|r| r == GATE),
            !open,
            "{path}: {reasons:?}"
        );
    }
    // Its own logout is open, and ends it.
    assert!(allowed(h.door(NOW + 1).check(&token, HOME, PLAIN)));
    assert!(send_as(&app, "POST", "/auth/logout", &token)
        .await
        .is_success());
    assert_eq!(
        h.door(NOW + 1).check(&token, HOME, PLAIN),
        AccessDecision::Deny
    );
    // A person's own session is not touched by the layer.
    let mut person = claims(&token, NOW);
    person.session = Some(ordo_identity::SessionClaim::starting_at(
        ordo_identity::Timestamp(NOW),
    ));
    let spec = ordo_identity::TokenSpec {
        principal: person.principal.clone(),
        expires: ordo_identity::Timestamp(NOW + 600),
        client: None,
        user_presence_at: None,
        capability: None,
        delegation: None,
        session: person.session.clone(),
    };
    let own = ordo_identity::token::mint(&TokenKey::from_secret(SECRET), &spec);
    let _ = send_as(&app, "GET", "/auth/authenticators", &own).await;
    assert!(!h.reasons().iter().any(|r| r == GATE));
}

/// Finding 1: a console session handed off would come back a normal
/// session, with a full lifetime and a renewal cookie, that revoking the
/// key would not end. The hand-off refuses it.
#[tokio::test(flavor = "multi_thread")]
async fn a_console_session_cannot_be_handed_off_for_a_full_session() {
    let h = harness();
    let app = h.app();
    let (sk, id) = h.enrol(1).await;
    let token = sign_in(&app, &sk, &id).await;
    let binder =
        ordo_identity::federation::handle_of("binder-console-0123456789abcdefghijklmnopqrstuvwxyz");
    let (status, body) = send(
        &app,
        "/auth/handoff",
        Some(json!({"return_to": APP, "binder_hash": binder})),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(body.get("redirect").is_none(), "{body}");
    assert!(h.reasons().iter().any(|r| r == GATE));
}

/// Finding 2: the token names no space, so the door binds a console
/// session to the deployment's own. The owner's own session still reaches
/// the tenant they own; a door told no home lets a console session reach
/// nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_console_session_reaches_the_home_space_and_no_other() {
    let h = harness();
    let app = h.app();
    let (sk, id) = h.enrol(1).await;
    let token = sign_in(&app, &sk, &id).await;
    let door = h.door(NOW + 1);
    assert!(allowed(door.check(&token, HOME, PLAIN)));
    assert_eq!(door.check(&token, OTHER, PLAIN), AccessDecision::Deny);
    let spec = ordo_identity::TokenSpec {
        principal: h.owner.clone(),
        expires: ordo_identity::Timestamp(NOW + 600),
        client: None,
        user_presence_at: None,
        capability: None,
        delegation: None,
        session: Some(ordo_identity::SessionClaim::starting_at(
            ordo_identity::Timestamp(NOW),
        )),
    };
    let own = ordo_identity::token::mint(&TokenKey::from_secret(SECRET), &spec);
    assert!(allowed(door.check(&own, OTHER, PLAIN)));
    assert!(door
        .take_audit()
        .iter()
        .any(|l| l.contains("ConsoleSessionOffHome")));
    // Unwired, it fails closed.
    let mut engine = AccessEngine::new();
    let space = SpaceId(HOME.into());
    engine.set_profile(&space, AccessProfile::new().allow(Role::Owner, PLAIN));
    engine.set_membership(&space, &h.owner, Role::Owner);
    let bare = IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW + 1),
        Arc::new(Metrics::new()),
    );
    assert_eq!(bare.check(&token, HOME, PLAIN), AccessDecision::Deny);
    assert!(allowed(bare.check(&own, HOME, PLAIN)));
}

/// Step 5: a console session reaches a tenant the owner owns for the
/// tenant's exit and bucket ops only, and only within 300 seconds of the
/// owner's relayed touch. The key alone reaches no tenant.
#[tokio::test(flavor = "multi_thread")]
async fn a_console_session_reaches_a_tenants_exit_ops_only_with_a_fresh_touch() {
    let h = harness();
    let app = h.app();
    let (sk, id) = h.enrol(1).await;
    let token = sign_in(&app, &sk, &id).await;
    // No touch: no tenant op, even one on the list.
    assert_eq!(
        h.door(NOW + 1).check(&token, OTHER, EXPORT),
        AccessDecision::Deny
    );
    h.at(NOW + 100);
    let (status, body, _) = relay_touch(&h, &app, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let present = body["token"].as_str().expect("token");
    let door = h.door(NOW + 101);
    assert!(allowed(door.check(present, OTHER, EXPORT)));
    // An op off the list stays in the home space, touch or not.
    assert_eq!(door.check(present, OTHER, PLAIN), AccessDecision::Deny);
    assert_eq!(door.check(present, OTHER, GATED), AccessDecision::Deny);
    assert!(door
        .take_audit()
        .iter()
        .any(|l| l.contains("ConsoleSessionOffHome")));
    // Past the window, the tenant is out of reach again.
    assert_eq!(
        h.door(NOW + 401).check(present, OTHER, EXPORT),
        AccessDecision::Deny
    );
    // The list is the one the console offers.
    assert_eq!(
        ordo_host::console_keys::TENANT_OPS_OPEN,
        [
            ordo_host::exit_ops::EXPORT,
            ordo_host::exit_ops::DELETE,
            ordo_host::bucket_ops::ATTACH_BUCKET,
            ordo_host::bucket_ops::DETACH_BUCKET,
            ordo_host::people_ops::MEMBERS,
            ordo_host::people_ops::GRANTS,
            ordo_host::people_ops::GROUP_MEMBERS,
            ordo_host::people_ops::CALLABLE_BY,
            ordo_host::profile_ops::GROUPS,
            ordo_host::platform_ops::CALLERS_OF,
            ordo_host::invite_ops::INVITE,
            ordo_host::member_ops::SET_ROLE,
            ordo_host::member_ops::REMOVE,
            ordo_host::profile_ops::ADD_TO_GROUP,
            ordo_host::share_links::LINKS,
            ordo_host::share_links::REVOKE_LINK,
            ordo_host::share_ops::SHARES,
            ordo_host::share_ops::REVOKE_SHARE,
            ordo_host::serve::SECEVENTS_OP,
            ordo_host::member_ops::SUSPEND,
            ordo_host::member_ops::REINSTATE,
            ordo_host::profile_ops::CREATE_GROUP,
            ordo_host::profile_ops::DELETE_GROUP,
            ordo_host::profile_ops::REMOVE_FROM_GROUP,
            ordo_host::profile_ops::PROFILE,
            ordo_host::profile_ops::SET_OP_ACCESS,
            ordo_host::profile_ops::RESET_OP_ACCESS,
            ordo_host::profile_ops::SET_MEMBER_ACCESS,
            ordo_host::invite_ops::INVITATIONS,
            ordo_host::invite_ops::REVOKE_INVITATION,
            ordo_host::admission::ADMISSION,
            ordo_host::admission::SET_ADMISSION,
            ordo_host::identity_ops::RECOVER_MEMBER,
            ordo_host::identity_ops::APPROVE_MEMBER_RECOVERY,
            ordo_host::identity_ops::SET_RECOVERY_POLICY,
        ]
    );
}

/// Step 8: the People reads take the same window. With a fresh touch a
/// console session reads a tenant's members where the owner holds a role;
/// never where the owner holds none, never with the key alone, never past
/// 300 seconds. A People write off the list (suspend) stays out of reach.
#[tokio::test(flavor = "multi_thread")]
async fn a_console_session_reads_a_tenants_people_only_with_a_fresh_touch_and_a_role() {
    let h = harness();
    let app = h.app();
    let (sk, id) = h.enrol(1).await;
    let token = sign_in(&app, &sk, &id).await;
    assert_eq!(
        h.door(NOW + 1).check(&token, OTHER, MEMBERS),
        AccessDecision::Deny,
        "the key alone"
    );
    h.at(NOW + 100);
    let (status, body, _) = relay_touch(&h, &app, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let present = body["token"].as_str().expect("token");
    let door = h.door(NOW + 101);
    assert!(allowed(door.check(present, OTHER, MEMBERS)));
    assert_eq!(
        door.check(present, NO_ROLE, MEMBERS),
        AccessDecision::Deny,
        "a tenant where the owner holds no role"
    );
    assert_eq!(
        door.check(present, OTHER, ordo_host::member_ops::SUSPEND),
        AccessDecision::Deny,
        "an op off the list"
    );
    assert_eq!(
        h.door(NOW + 401).check(present, OTHER, MEMBERS),
        AccessDecision::Deny,
        "past the window"
    );
}

/// Finding 3: a console begin stores nothing, so a stranger who names any
/// key id writes no row. Begins and completes count against the node's
/// throttle.
#[tokio::test(flavor = "multi_thread")]
async fn a_console_begin_stores_nothing_and_sign_in_attempts_are_throttled() {
    use ordo_host::console_keys::{ATTEMPT_WINDOW_SECS, MAX_ATTEMPTS_PER_WINDOW};
    let h = harness();
    let app = h.app();
    let before = h.ceremonies.len();
    for i in 0..MAX_ATTEMPTS_PER_WINDOW {
        begin(&app, &format!("ck-{i:032x}")).await;
    }
    assert_eq!(h.ceremonies.len(), before, "a begin wrote a row");
    let (status, _) = send(
        &app,
        "/auth/begin",
        Some(json!({"method": "console", "key": format!("ck-{:032x}", 99)})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(h
        .reasons()
        .contains(&"console-sign-in-throttled".to_string()));
    h.at(NOW + ATTEMPT_WINDOW_SECS);
    let (sk, id) = h.enrol(1).await;
    sign_in(&app, &sk, &id).await;
}

/// Finding 5: a revocation that lands after the sign-in's live check and
/// before its session row is written would find no session to end. The
/// sign-in writes the row first and looks again.
#[tokio::test(flavor = "multi_thread")]
async fn a_revocation_that_lands_during_a_sign_in_mints_nothing() {
    let target: Arc<std::sync::Mutex<Option<String>>> = Arc::default();
    let t = target.clone();
    let store = ConsoleKeyStore::memory_with_sign_in_hook(Box::new(move |keys| {
        let id = t.lock().unwrap().clone();
        for k in keys.iter_mut().filter(|k| Some(&k.id) == id.as_ref()) {
            k.revoked_at = Some(NOW);
        }
    }));
    let h = harness_with(ordo_host::console_keys::DEFAULT_SESSION_SECS, store);
    let app = h.app();
    let (sk, id) = h.enrol(1).await;
    *target.lock().unwrap() = Some(id.clone());
    let ch = begin(&app, &id).await;
    let (status, body) = complete(&app, &id, &ch, &sign(&sk, &ch)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(body.get("token").is_none());
    assert!(h.reasons().contains(&"console-key-revoked".to_string()));
}

// ═════════════════════════════ G16 · boot record ══════════════════════════

#[test]
fn the_boot_record_names_the_revision_and_who_applied_it_in_the_security_record() {
    use ordo_host::boot_record::{BootRecord, APPLIED_BY_ENV, REVISION_ENV};
    let vars = vec![
        (REVISION_ENV.to_string(), "12".to_string()),
        (APPLIED_BY_ENV.to_string(), "owner:own_1".to_string()),
        ("ORDO_PG_PASSWORD".to_string(), "hunter2".to_string()),
    ];
    let record = BootRecord::from_vars(&vars, Some("abc".into()), NOW);
    let stream = ordo_secevent::Recorder::default();
    ordo_host::boot_record::install(record.clone(), &stream);
    let entries = stream.drain_all(ordo_commandlog::Timestamp(NOW + 1));
    let detail = entries
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::DeploymentBooted { detail } => Some(detail.clone()),
            _ => None,
        })
        .expect("a boot record");
    assert!(
        detail.starts_with("revision=12 applied_by=owner:own_1 "),
        "{detail}"
    );
    assert!(!detail.contains("hunter2"), "never a secret's value");
    let shown = ordo_host::boot_record::current_json();
    assert_eq!(shown["revision"], "12");
    assert_eq!(shown["applied_by"], "owner:own_1");
    assert_eq!(shown["settings_fingerprint"], record.settings_fingerprint);
}

// ═════════════════════ the store, on the control database ═════════════════

/// The Postgres store holds the same rules as the memory one: two live keys
/// an owner, one row a public key, revocation once, sessions until they
/// expire. Needs the conformance cluster.
#[tokio::test(flavor = "multi_thread")]
async fn the_postgres_store_keeps_the_same_rules() {
    use ordo_host::console_keys::KeyRow;
    let c = ordo_exec::PoolConfig::from_env();
    let db = "ordo_host_t_console_keys";
    ordo_exec::drop_database(&c, db).await.expect("drop");
    ordo_exec::create_database(&c, db).await.expect("create");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    let store = ConsoleKeyStore::pg(pool);
    store.ensure_tables().await.expect("tables");
    store.ensure_tables().await.expect("twice is fine");
    let row = |id: &str, pk: &str| KeyRow {
        id: id.into(),
        principal: "usr_ana".into(),
        name: "console".into(),
        public_key: pk.into(),
        created_at: NOW,
        last_used_at: None,
        revoked_at: None,
        revoked_by: None,
    };
    assert!(store.insert(&row("ck-1", "pk1")).await.unwrap());
    assert!(
        !store.insert(&row("ck-x", "pk1")).await.unwrap(),
        "one row a public key"
    );
    assert!(store.insert(&row("ck-2", "pk2")).await.unwrap());
    assert!(
        !store.insert(&row("ck-3", "pk3")).await.unwrap(),
        "two live keys an owner"
    );
    store.signed_in("ck-1", "s1", NOW + 900, NOW).await.unwrap();
    assert_eq!(
        store.get("ck-1").await.unwrap().unwrap().last_used_at,
        Some(NOW)
    );
    assert_eq!(
        store.sessions_of("ck-1", NOW + 1).await.unwrap(),
        vec![("s1".to_string(), NOW + 900)]
    );
    assert!(store
        .sessions_of("ck-1", NOW + 900)
        .await
        .unwrap()
        .is_empty());
    let revoked = store
        .revoke("ck-1", "usr_ana", NOW + 5)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(revoked.revoked_at, Some(NOW + 5));
    assert!(store
        .revoke("ck-1", "usr_ana", NOW + 6)
        .await
        .unwrap()
        .is_none());
    assert!(
        store.insert(&row("ck-3", "pk3")).await.unwrap(),
        "room again"
    );
    let ids: Vec<String> = store
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|k| k.id)
        .collect();
    assert_eq!(ids, vec!["ck-1", "ck-2", "ck-3"]);
    // A challenge is spent once, and a spent one past its expiry is dropped.
    assert!(store.spend("h1", NOW + 60, NOW).await.unwrap());
    assert!(!store.spend("h1", NOW + 60, NOW + 1).await.unwrap());
    assert!(store.spend("h2", NOW + 120, NOW + 60).await.unwrap());
    assert!(
        store.spend("h1", NOW + 200, NOW + 61).await.unwrap(),
        "dropped once expired"
    );
}

/// With `ORDO_IDENTITY_URL` set, an enrolled console key does not sign in,
/// at either leg, and the refusal names the setting
/// (`plan-identity-service.md` 6, G14).
#[tokio::test(flavor = "multi_thread")]
async fn a_console_key_does_not_sign_in_while_the_identity_service_is_set() {
    // The key was enrolled before the identity service was set.
    let store = ConsoleKeyStore::memory();
    let before = harness_with(ordo_host::console_keys::DEFAULT_SESSION_SECS, store.clone());
    let (sk, id) = before.enrol(1).await;
    let h = harness_full(
        ordo_host::console_keys::DEFAULT_SESSION_SECS,
        store,
        Some(ordo_host::identity_keys::URL_ENV),
    );
    let app = h.app();
    let (status, body) = send(
        &app,
        "/auth/begin",
        Some(json!({"method": "console", "key": id})),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(body.to_string().contains("ORDO_IDENTITY_URL"), "{body}");
    // A challenge minted while sign-in was open cannot be completed now.
    let open = harness();
    let ch = begin(&open.app(), &id).await;
    let (status, body) = complete(&app, &id, &ch, &sign(&sk, &ch)).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(body.to_string().contains("ORDO_IDENTITY_URL"), "{body}");
    assert!(h
        .reasons()
        .iter()
        .all(|r| r == "console-key-closed-identity-service"));
}

/// Step 8: with `ORDO_IDENTITY_URL` set no console key is made, and the
/// refusal names the setting. A key left from before is still listed and
/// can still be revoked, so it can be ended.
#[tokio::test(flavor = "multi_thread")]
async fn no_console_key_is_made_while_the_identity_service_is_set_and_an_old_one_can_be_revoked() {
    let store = ConsoleKeyStore::memory();
    let before = harness_with(ordo_host::console_keys::DEFAULT_SESSION_SECS, store.clone());
    let (_, old) = before.enrol(1).await;
    let h = harness_full(
        ordo_host::console_keys::DEFAULT_SESSION_SECS,
        store,
        Some(ordo_host::identity_keys::URL_ENV),
    );
    let sk = SigningKey::from_bytes(&[2u8; 32]);
    let pk = b64url_encode(sk.verifying_key().as_bytes());
    let e = h
        .op(ADD, json!({"publicKey": pk, "name": "after the cut-over"}))
        .await
        .expect_err("no key is made");
    assert!(e.to_string().contains("ORDO_IDENTITY_URL"), "{e}");
    let listed = h.op(LIST, json!({})).await.expect("listed");
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    assert_eq!(listed[0]["id"].as_str(), Some(old.as_str()));
    h.op(REVOKE, json!({"id": old})).await.expect("revoked");
}
