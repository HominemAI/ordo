//! The door's other side: a caller with no token gets one, or gets nothing
//! and learns nothing from which nothing it got.
//!
//! Before `ordo_host::auth`, `ordo_identity::token::mint` had no caller
//! outside tests and no route began or completed a ceremony. Every
//! authorization surface verified a bearer token and nothing in the tree
//! could issue one, so a deployment refused every request correctly and
//! admitted none. These tests are the end to end that was missing.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{AccessChecker, AccessDecision};
use ordo_host::auth::AuthState;
use ordo_host::metrics::Metrics;
use ordo_host::IdentityAccess;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::provider::{Argon2idHasher, LocalPasswordProvider};
use ordo_identity::AccessEngine;
use ordo_identity::{MemorySessionRevocations, ProviderRegistry, TokenKey};
use ordo_secevent::{EventKind, RecordedDecision};
use serde_json::Value;
use tower::ServiceExt;

const SECRET: &[u8] = b"auth-door-test-secret";

/// A deployment with one password provider holding one enrolled credential,
/// and a clock pinned so the token's expiry is arithmetic rather than luck.
fn auth() -> Arc<AuthState> {
    auth_at(1_000).0
}

/// The same, with the clock pinned to `at` and the deny list handed back so
/// a test can ask the door about it.
fn auth_at(at: u64) -> (Arc<AuthState>, Arc<MemorySessionRevocations>) {
    let revocations = Arc::new(MemorySessionRevocations::new());
    (auth_sharing(revocations.clone(), at), revocations)
}

/// The same routes, a different clock, **the same deny list**, which is
/// how a test plays "later" without a sleep: one process signs in at one
/// instant and another answers at a later one, over shared state.
fn auth_sharing(revocations: Arc<MemorySessionRevocations>, at: u64) -> Arc<AuthState> {
    let provider = LocalPasswordProvider::new(Argon2idHasher::weak_for_tests());
    provider
        .enroll("ana@example.test", "correct-horse")
        .expect("enrolled");
    let mut registry = ProviderRegistry::new();
    registry
        .register("password", Box::new(provider))
        .expect("one provider");
    let mut state = AuthState::new(
        registry,
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations)
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()));
    state.now = Box::new(move || at);
    Arc::new(state)
}

/// Sign in and return the token, through the real routes.
async fn sign_in(app: &axum::Router) -> String {
    let (_, begun) = post(
        app,
        "/auth/begin",
        serde_json::json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let challenge = begun["challenge"]
        .as_str()
        .expect("a challenge")
        .to_string();
    let (status, done) = post(
        app,
        "/auth/complete",
        serde_json::json!({
            "method": "password", "kind": "password", "challenge": challenge,
            "identifier": "ana@example.test", "password": "correct-horse",
            "display": "Ana"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done["token"].as_str().expect("a token").to_string()
}

async fn post_bearer(app: &axum::Router, path: &str, token: &str) -> (StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
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

async fn post(app: &axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// **A caller with no token can get one, and it verifies under the key the
/// door checks with.** The whole point: mint and verify must agree, because
/// a deployment minting under one key and verifying under another
/// authenticates nobody and does it silently.
#[tokio::test(flavor = "multi_thread")]
async fn a_password_ceremony_mints_a_token_the_door_accepts() {
    let app = ordo_host::auth::router(Some(auth()));

    let (status, begun) = post(
        &app,
        "/auth/begin",
        serde_json::json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let challenge = begun["challenge"]
        .as_str()
        .expect("a challenge")
        .to_string();

    let (status, done) = post(
        &app,
        "/auth/complete",
        serde_json::json!({
            "method": "password", "kind": "password", "challenge": challenge,
            "identifier": "ana@example.test", "password": "correct-horse",
            "display": "Ana"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let token = done["token"].as_str().expect("a token");
    assert_eq!(done["expires_at"], 1_000 + 3600);

    // The door's own verifier, under the same key.
    let claims = ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        token,
        ordo_identity::Timestamp(1_100),
    )
    .expect("the door accepts what the door minted");
    assert!(!claims.principal.0.is_empty());

    // And a different key does not, which is what makes the assertion
    // above about agreement rather than about parsing.
    assert!(ordo_identity::verify(
        &TokenKey::from_secret(b"a different deployment"),
        token,
        ordo_identity::Timestamp(1_100),
    )
    .is_err());
}

/// **Every failure is the same failure.** An unknown identifier, a wrong
/// password, a stale challenge and a method nobody registered must be
/// indistinguishable, or the route is an account-existence oracle and a
/// stranger enumerates the directory by watching which complaint changes.
#[tokio::test(flavor = "multi_thread")]
async fn every_refusal_is_the_same_refusal() {
    let app = ordo_host::auth::router(Some(auth()));
    let (_, begun) = post(
        &app,
        "/auth/begin",
        serde_json::json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let challenge = begun["challenge"]
        .as_str()
        .expect("a challenge")
        .to_string();

    let wrong_password = serde_json::json!({
        "method": "password", "kind": "password", "challenge": challenge,
        "identifier": "ana@example.test", "password": "not-it"});
    let unknown_identifier = serde_json::json!({
        "method": "password", "kind": "password", "challenge": "cha_absent",
        "identifier": "nobody@example.test", "password": "correct-horse"});
    let unknown_method = serde_json::json!({
        "method": "carrier-pigeon", "kind": "password", "challenge": "cha_absent",
        "identifier": "ana@example.test", "password": "correct-horse"});
    let malformed = serde_json::json!({"method": "password"});

    let mut seen = Vec::new();
    for body in [
        wrong_password,
        unknown_identifier,
        unknown_method,
        malformed,
    ] {
        let (status, json) = post(&app, "/auth/complete", body).await;
        seen.push((status, json.to_string()));
    }
    let first = seen[0].clone();
    for (status, json) in &seen {
        assert_eq!(
            (*status, json.clone()),
            first,
            "two refusals differ, so the route tells a stranger which of \
             their guesses was closer"
        );
    }
    assert_eq!(first.0, StatusCode::UNAUTHORIZED);
}

/// **A deployment with no provider says so by name.** Not 404: a surface
/// missing because nobody wired it must not look like one that does not
/// exist, which is the argument the drain's `wired: false` already makes.
#[tokio::test(flavor = "multi_thread")]
async fn an_unwired_deployment_says_so() {
    let app = ordo_host::auth::router(None);
    let (status, json) = post(
        &app,
        "/auth/begin",
        serde_json::json!({"method": "password"}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    assert_eq!(json["error"]["code"], "auth_unconfigured");
}

// ═══════════════════ the rest of a session's life ════════════════════════

/// **A logged-out token is refused by the door**, while the same token a
/// moment earlier was not.
///
/// This is the assertion the platform could not make at all: the only
/// revocation it had was suspending the principal, which ends every session
/// on every device and is a membership change rather than a session one.
/// The door's answer is the *recorded decision*, because the wire answer is
/// one uniform `Deny` by design, `SessionRevoked` against `NoMembership`
/// is what shows the revocation check fired, and fired **before** the
/// engine was asked anything.
#[tokio::test(flavor = "multi_thread")]
async fn a_logged_out_token_is_refused_by_the_door() {
    let (auth, revocations) = auth_at(1_000);
    let app = ordo_host::auth::router(Some(auth));
    let token = sign_in(&app).await;

    // The door, wired to the same deny list the routes write.
    let door = IdentityAccess::new(
        AccessEngine::new(),
        TokenKey::from_secret(SECRET),
        Box::new(|| 1_100),
        Arc::new(Metrics::new()),
    )
    .with_revocations(revocations.clone());

    // Before the logout the token is refused for want of a membership,
    // which is a fact about this bare engine and not about the session.
    assert_eq!(
        door.check(&token, "spc_1", "openTasks"),
        AccessDecision::Deny
    );
    let before = door.take_events();
    assert_eq!(
        before[0].kind,
        EventKind::denied(RecordedDecision::NoMembership),
        "before the logout the session is not what stops it"
    );

    let (status, body) = post_bearer(&app, "/auth/logout", &token).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(revocations.len(), 1, "the session is on the deny list");

    assert_eq!(
        door.check(&token, "spc_1", "openTasks"),
        AccessDecision::Deny
    );
    let after = door.take_events();
    assert_eq!(
        after[0].kind,
        EventKind::denied(RecordedDecision::SessionRevoked),
        "after the logout the session is exactly what stops it"
    );
}

/// A token minted for a **member** is admitted, and the same token after a
/// logout is not. The half above cannot show this, because a bare engine
/// denies either way; this is the one that proves a logout takes away
/// access somebody actually had.
#[tokio::test(flavor = "multi_thread")]
async fn a_logout_takes_away_access_that_worked() {
    let (auth, revocations) = auth_at(1_000);
    let app = ordo_host::auth::router(Some(auth));
    let token = sign_in(&app).await;

    let claims = ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        &token,
        ordo_identity::Timestamp(1_100),
    )
    .expect("a token this deployment minted");

    let space = ordo_identity::SpaceId("spc_1".into());
    let mut engine = AccessEngine::new();
    engine.set_membership(&space, &claims.principal, ordo_identity::Role::Editor);
    engine.set_profile(
        &space,
        ordo_identity::AccessProfile::new().allow(ordo_identity::Role::Editor, "openTasks"),
    );
    let door = IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(|| 1_100),
        Arc::new(Metrics::new()),
    )
    .with_revocations(revocations.clone());

    assert_eq!(
        door.check(&token, "spc_1", "openTasks"),
        AccessDecision::Allow {
            principal: claims.principal.0.clone(),
            agent: None,
        },
        "a member's token opens the op it is allowed"
    );

    let (status, _) = post_bearer(&app, "/auth/logout", &token).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert_eq!(
        door.check(&token, "spc_1", "openTasks"),
        AccessDecision::Deny,
        "the same token, the same membership, and no longer admitted"
    );
}

/// **A session continues without re-authenticating**, and the token that
/// comes back is a different string that verifies under the same key.
#[tokio::test(flavor = "multi_thread")]
async fn refresh_continues_a_session_without_a_ceremony() {
    let (auth, revocations) = auth_at(1_000);
    let app = ordo_host::auth::router(Some(auth));
    let first = sign_in(&app).await;

    // Half an hour on, over the same deny list: no ceremony, no password.
    let later = ordo_host::auth::router(Some(auth_sharing(revocations, 2_800)));
    let (status, body) = post_bearer(&later, "/auth/refresh", &first).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let second = body["token"].as_str().expect("a token");
    assert_ne!(second, first, "a refresh mints a new token");
    assert_eq!(
        body["expires_at"],
        2_800 + 3_600,
        "the new token expires an hour from now, not an hour from the login"
    );
    assert_eq!(
        body["session_expires_at"],
        1_000 + 12 * 3_600,
        "the ceiling is counted from the sign-in and does not move"
    );

    let claims = ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        second,
        ordo_identity::Timestamp(2_900),
    )
    .expect("the refreshed token verifies under the door's key");
    let original = ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        &first,
        ordo_identity::Timestamp(2_900),
    )
    .expect("so does the original");
    assert_eq!(
        claims.session, original.session,
        "the same session, continued rather than restarted"
    );
    assert_eq!(claims.principal, original.principal);
}

/// A logged-out session cannot be continued either, which is what stops a
/// logout from being a one-request inconvenience.
#[tokio::test(flavor = "multi_thread")]
async fn a_logged_out_session_cannot_be_refreshed() {
    let (auth, _) = auth_at(1_000);
    let app = ordo_host::auth::router(Some(auth));
    let token = sign_in(&app).await;

    let (status, _) = post_bearer(&app, "/auth/logout", &token).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, json) = post_bearer(&app, "/auth/refresh", &token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "authentication_failed");
}

/// A session past its ceiling is over. The token still verifies: it was
/// refreshed inside the ceiling and has an hour on it, and refreshing it
/// again is refused, so the holder has to run a ceremony.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_ends_at_its_ceiling() {
    let (auth, revocations) = auth_at(1_000);
    let app = ordo_host::auth::router(Some(auth));
    let token = sign_in(&app).await;

    // Thirteen hours on: past the twelve-hour ceiling.
    let later = ordo_host::auth::router(Some(auth_sharing(revocations, 1_000 + 13 * 3_600)));
    let (status, json) = post_bearer(&later, "/auth/refresh", &token).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a session past its ceiling cannot be continued: {json}"
    );
}

/// **The logout route is not an oracle either.** A garbage token, a token
/// from another deployment and a real one are the same `204`, so whoever is
/// holding a stolen string learns nothing about whether it was worth
/// anything.
#[tokio::test(flavor = "multi_thread")]
async fn every_logout_is_the_same_answer() {
    let (auth, _) = auth_at(1_000);
    let app = ordo_host::auth::router(Some(auth));
    let real = sign_in(&app).await;
    let foreign = ordo_identity::mint(
        &TokenKey::from_secret(b"a different deployment"),
        &ordo_identity::TokenSpec {
            principal: ordo_identity::PrincipalId("usr_elsewhere".into()),
            expires: ordo_identity::Timestamp(9_999),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: Some(ordo_identity::SessionClaim::starting_at(
                ordo_identity::Timestamp(1_000),
            )),
        },
    );

    let mut seen = Vec::new();
    for token in [real.as_str(), foreign.as_str(), "junk", ""] {
        let (status, json) = post_bearer(&app, "/auth/logout", token).await;
        seen.push((status, json.to_string()));
    }
    for answer in &seen {
        assert_eq!(
            answer, &seen[0],
            "two logout answers differ, so the route says which strings were real"
        );
    }
    assert_eq!(seen[0].0, StatusCode::NO_CONTENT);
}

/// Logging out twice is logging out. A client retrying a failed request
/// must not be told the second attempt was an error.
#[tokio::test(flavor = "multi_thread")]
async fn logging_out_twice_is_logging_out() {
    let (auth, revocations) = auth_at(1_000);
    let app = ordo_host::auth::router(Some(auth));
    let token = sign_in(&app).await;

    for _ in 0..2 {
        let (status, _) = post_bearer(&app, "/auth/logout", &token).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    assert_eq!(revocations.len(), 1);
}

/// A deployment with no deny list has no logout, and says so rather than
/// answering `204` for something it cannot do. A client that trusted that
/// `204` would discard a token the door goes on honouring.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_with_no_deny_list_does_not_pretend_to_log_out() {
    let provider = LocalPasswordProvider::new(Argon2idHasher::weak_for_tests());
    provider
        .enroll("ana@example.test", "correct-horse")
        .expect("enrolled");
    let mut registry = ProviderRegistry::new();
    registry
        .register("password", Box::new(provider))
        .expect("one provider");
    let mut state = AuthState::new(
        registry,
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    );
    state.now = Box::new(|| 1_000);
    let app = ordo_host::auth::router(Some(Arc::new(state)));
    let token = sign_in(&app).await;

    let (status, json) = post_bearer(&app, "/auth/logout", &token).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    assert_eq!(json["error"]["code"], "auth_unconfigured");
}

/// The stream a test reads: everything the routes recorded so far.
fn recorded(auth: &AuthState) -> Vec<ordo_secevent::StreamEntry> {
    auth.recorder
        .as_ref()
        .expect("the harness wires a recorder")
        .drain_all(ordo_commandlog::Timestamp(2_000))
}

fn failure_reasons(entries: &[ordo_secevent::StreamEntry]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::AuthenticationFailed { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

/// **The caller is told one thing and the operator is told which.** Two
/// different failures answer with the same status and body, and the
/// security stream carries a different reason for each, each with no
/// principal named, because a refusal proves nobody.
#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_is_one_answer_to_the_caller_and_a_reason_to_the_operator() {
    let auth = auth();
    let app = ordo_host::auth::router(Some(auth.clone()));
    let (_, begun) = post(
        &app,
        "/auth/begin",
        serde_json::json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let challenge = begun["challenge"].as_str().expect("a challenge");
    let wrong_password = post(
        &app,
        "/auth/complete",
        serde_json::json!({
            "method": "password", "kind": "password", "challenge": challenge,
            "identifier": "ana@example.test", "password": "not-it"}),
    )
    .await;
    let unknown_method = post(
        &app,
        "/auth/complete",
        serde_json::json!({
            "method": "carrier-pigeon", "kind": "password", "challenge": "cha_absent",
            "identifier": "ana@example.test", "password": "correct-horse"}),
    )
    .await;
    assert_eq!(
        (wrong_password.0, wrong_password.1.to_string()),
        (unknown_method.0, unknown_method.1.to_string()),
        "the caller must not be able to tell the two apart"
    );
    assert_eq!(wrong_password.0, StatusCode::UNAUTHORIZED);

    let entries = recorded(&auth);
    assert_eq!(
        failure_reasons(&entries),
        vec!["bad-credentials".to_string(), "unknown-method".to_string()],
        "the stream says which"
    );
    for e in &entries {
        assert_eq!(e.subject, ordo_secevent::Subject::Unverified);
    }
}

/// **An onboarding is itself an audited event** (`security-logging-design.md`
/// §3). A first sign-in mints a principal and links its authenticator, and
/// before this the two audit events were returned by `sign_in` and dropped
/// by the route.
#[tokio::test(flavor = "multi_thread")]
async fn an_onboarding_is_in_the_security_stream() {
    let auth = auth();
    let app = ordo_host::auth::router(Some(auth.clone()));
    let _ = sign_in(&app).await;

    let entries = recorded(&auth);
    let kinds: Vec<&str> = entries.iter().map(|e| e.kind.label()).collect();
    assert_eq!(
        kinds,
        vec![
            "principal-onboarded",
            "authenticator-linked",
            "authenticated"
        ],
        "{kinds:?}"
    );
    let linked = entries
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::AuthenticatorLinked { added, vouched_by } => {
                Some((added.clone(), vouched_by.clone()))
            }
            _ => None,
        })
        .expect("the link is recorded");
    assert!(linked.0.starts_with("password:"), "{linked:?}");
    assert_eq!(linked.1, None, "the first login is the bootstrap");
    let who = entries[0].subject.clone();
    assert!(matches!(who, ordo_secevent::Subject::Attributed { .. }));
    assert!(
        entries.iter().all(|e| e.subject == who),
        "all three name the one principal"
    );
}

/// The refusals of `/auth/refresh` and `/auth/logout` are recorded too,
/// while the caller still gets the one answer each gave before.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_refresh_and_a_refused_logout_are_recorded() {
    let auth = auth();
    let app = ordo_host::auth::router(Some(auth.clone()));
    let (status, _) = post_bearer(&app, "/auth/refresh", "not-a-token").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = post_bearer(&app, "/auth/logout", "not-a-token").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        failure_reasons(&recorded(&auth)),
        vec![
            "token-unverified".to_string(),
            "token-unverified".to_string()
        ]
    );
}

// ───────────── recovery R1: ending every session of a principal ─────────

/// A door whose engine gives `token`'s principal the editor role on
/// `spc_1`, so an admitted token and a refused one differ only by the
/// session's end.
fn member_door(token: &str, revocations: Arc<MemorySessionRevocations>, at: u64) -> IdentityAccess {
    let claims = ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        token,
        ordo_identity::Timestamp(at),
    )
    .expect("a token this deployment minted");
    let space = ordo_identity::SpaceId("spc_1".into());
    let mut engine = AccessEngine::new();
    engine.set_membership(&space, &claims.principal, ordo_identity::Role::Editor);
    engine.set_profile(
        &space,
        ordo_identity::AccessProfile::new().allow(ordo_identity::Role::Editor, "openTasks"),
    );
    IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(move || at),
        Arc::new(Metrics::new()),
    )
    .with_revocations(revocations)
}

/// **One call ends every session the person has, on every device.** Two
/// sign-ins, one call, both tokens refused, and a sign-in afterwards
/// accepted.
#[tokio::test(flavor = "multi_thread")]
async fn ending_every_session_refuses_each_token_minted_before_it() {
    let revocations = Arc::new(MemorySessionRevocations::new());
    let app = ordo_host::auth::router(Some(auth_sharing(revocations.clone(), 1_000)));
    let phone = sign_in(&app).await;
    let laptop = sign_in(&app).await;

    let door = member_door(&phone, revocations.clone(), 1_100);
    for t in [&phone, &laptop] {
        assert!(
            matches!(
                door.check(t, "spc_1", "openTasks"),
                AccessDecision::Allow { .. }
            ),
            "both sessions work before the call"
        );
    }

    // The laptop ends everything, five hundred seconds on.
    let later = ordo_host::auth::router(Some(auth_sharing(revocations.clone(), 1_500)));
    let (status, body) = post_bearer(&later, "/auth/logout-all", &laptop).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let door = member_door(&phone, revocations.clone(), 1_600);
    for t in [&phone, &laptop] {
        assert_eq!(
            door.check(t, "spc_1", "openTasks"),
            AccessDecision::Deny,
            "a token minted before the call is refused"
        );
    }
    let events = door.take_events();
    assert!(
        events
            .iter()
            .all(|e| e.kind == EventKind::denied(RecordedDecision::SessionRevoked)),
        "it is the ended session that stops them, not the membership"
    );

    // A sign-in after the call starts a session the call does not touch.
    let after = ordo_host::auth::router(Some(auth_sharing(revocations.clone(), 1_600)));
    let fresh = sign_in(&after).await;
    let door = member_door(&fresh, revocations, 1_700);
    assert!(
        matches!(
            door.check(&fresh, "spc_1", "openTasks"),
            AccessDecision::Allow { .. }
        ),
        "a session that starts after the call is admitted"
    );
}

/// An ended session is over: it cannot be continued into a new one.
#[tokio::test(flavor = "multi_thread")]
async fn an_ended_session_cannot_be_refreshed() {
    let revocations = Arc::new(MemorySessionRevocations::new());
    let app = ordo_host::auth::router(Some(auth_sharing(revocations.clone(), 1_000)));
    let stolen = sign_in(&app).await;
    let owner = sign_in(&app).await;

    let later = ordo_host::auth::router(Some(auth_sharing(revocations, 1_500)));
    let (status, _) = post_bearer(&later, "/auth/logout-all", &owner).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, json) = post_bearer(&later, "/auth/refresh", &stolen).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "authentication_failed");
}

/// The route is refused for what is not a live session, the stream says
/// which, and the call is recorded when it works.
#[tokio::test(flavor = "multi_thread")]
async fn logout_all_is_refused_without_a_live_session_and_recorded_when_it_works() {
    let auth = auth();
    let app = ordo_host::auth::router(Some(auth.clone()));
    let (status, _) = post_bearer(&app, "/auth/logout-all", "not-a-token").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let token = sign_in(&app).await;
    let (status, _) = post_bearer(&app, "/auth/logout-all", &token).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // The session it ended cannot end anything again.
    let (status, _) = post_bearer(&app, "/auth/logout-all", &token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let entries = recorded(&auth);
    assert_eq!(
        failure_reasons(&entries),
        vec!["token-unverified".to_string(), "session-ended".to_string()]
    );
    assert!(
        entries.iter().any(|e| e.kind
            == EventKind::SessionsEnded {
                reason: "logout-all".into()
            }),
        "the call is in the stream"
    );
}

// ── Staying signed in across a reload (owner ruling 2026-10-21) ─────────────

use ordo_host::renewal::{MemoryRenewals, RememberConfig, RenewingRevocations};

/// Stores shared between "processes" at different clocks, as `auth_sharing`
/// shares the deny list.
#[derive(Clone)]
struct Kept {
    revocations: Arc<MemorySessionRevocations>,
    renewals: Arc<MemoryRenewals>,
}

impl Kept {
    fn new() -> Kept {
        Kept {
            revocations: Arc::new(MemorySessionRevocations::new()),
            renewals: Arc::new(MemoryRenewals::new()),
        }
    }

    /// The routes at `at`, with renewals as the deployment configures them.
    fn at(&self, at: u64, config: RememberConfig) -> axum::Router {
        let provider = LocalPasswordProvider::new(Argon2idHasher::weak_for_tests());
        provider
            .enroll("ana@example.test", "correct-horse")
            .expect("enrolled");
        let mut registry = ProviderRegistry::new();
        registry
            .register("password", Box::new(provider))
            .expect("one provider");
        let mut state = AuthState::new(
            registry,
            Directory::new(Provisioning::AutoOnboard),
            TokenKey::from_secret(SECRET),
        )
        .with_revocations(Arc::new(RenewingRevocations::new(
            self.revocations.clone(),
            self.renewals.clone(),
        )))
        .with_remember(ordo_host::renewal::Remember {
            store: self.renewals.clone(),
            config,
            app_origins: app_origins(),
        })
        .with_recorder(Arc::new(ordo_secevent::Recorder::default()));
        state.now = Box::new(move || at);
        ordo_host::auth::router(Some(Arc::new(state)))
    }
}

struct Answer {
    status: StatusCode,
    json: Value,
    cookie: Option<String>,
}

async fn send(app: &axum::Router, req: Request<Body>) -> Answer {
    let res = app.clone().oneshot(req).await.expect("response");
    let status = res.status();
    let cookie = res
        .headers()
        .get("set-cookie")
        .map(|v| v.to_str().expect("ascii").to_string());
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    Answer {
        status,
        json: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        cookie,
    }
}

/// Sign in through the real routes from `host`; the token and `Set-Cookie`.
async fn sign_in_with(
    app: &axum::Router,
    host: &str,
    remember: Option<bool>,
) -> (String, Option<String>) {
    let (_, begun) = post(
        app,
        "/auth/begin",
        serde_json::json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let mut body = serde_json::json!({
        "method": "password", "kind": "password",
        "challenge": begun["challenge"].as_str().expect("a challenge"),
        "identifier": "ana@example.test", "password": "correct-horse",
    });
    if let Some(r) = remember {
        body["remember"] = serde_json::json!(r);
    }
    let a = send(
        app,
        Request::builder()
            .method("POST")
            .uri("/auth/complete")
            .header("host", host)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.json);
    (
        a.json["token"].as_str().expect("token").to_string(),
        a.cookie,
    )
}

async fn sign_in_from(app: &axum::Router, host: &str) -> (String, Option<String>) {
    sign_in_with(app, host, None).await
}

/// The secret in a `Set-Cookie`.
fn secret_of(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .and_then(|p| {
            p.strip_prefix("__Secure-ordo_renew=")
                .or_else(|| p.strip_prefix("ordo_renew="))
        })
        .expect("the renewal cookie")
        .to_string()
}

/// The app's page, on the deployment's list (`ORDO_CORS_ORIGINS`).
const APP: &str = "http://localhost:5173";

fn app_origins() -> Option<ordo_api::cors::CorsPolicy> {
    ordo_api::cors::CorsPolicy::parse(APP).expect("an origin")
}

/// A page load: `/auth/refresh` with the cookie and no bearer token, from
/// the app's page.
async fn reload(app: &axum::Router, secret: &str) -> Answer {
    reload_from(app, secret, Some(APP)).await
}

/// [`reload`], from the page at `origin`, or with no `Origin` at all.
async fn reload_from(app: &axum::Router, secret: &str, origin: Option<&str>) -> Answer {
    let mut b = Request::builder()
        .method("POST")
        .uri("/auth/refresh")
        .header("host", "127.0.0.1:4000")
        .header("cookie", format!("ordo_renew={secret}"));
    if let Some(o) = origin {
        b = b.header("origin", o);
    }
    send(app, b.body(Body::empty()).expect("request")).await
}

/// Only the app's own page redeems the cookie. Script on the door's own
/// origin (a file served from it), on a foreign site, or a request with no
/// `Origin` is refused, the refusal names the setting, and the cookie is
/// neither spent nor cleared.
#[tokio::test(flavor = "multi_thread")]
async fn only_an_app_origin_redeems_the_renewal_cookie() {
    let kept = Kept::new();
    let (_, cookie) = sign_in_from(&kept.at(1_000, ON), "127.0.0.1:4000").await;
    let secret = secret_of(&cookie.expect("cookie"));
    let app = kept.at(2_000, ON);
    for origin in [
        Some("http://127.0.0.1:4000"),
        Some("https://evil.example"),
        Some("null"),
        None,
    ] {
        let r = reload_from(&app, &secret, origin).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{origin:?}");
        assert_eq!(r.json["error"]["code"], "remember_off", "{origin:?}");
        assert!(
            r.json["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("ORDO_CORS_ORIGINS"),
            "{origin:?}: {}",
            r.json
        );
        assert!(r.json.get("token").is_none(), "{origin:?}");
        assert!(r.cookie.is_none(), "{origin:?}: the cookie is left alone");
    }
    // Not spent: the app's page still turns it, and gets a fresh secret
    // (a spent one would answer in its grace window with no new cookie).
    let a = reload(&app, &secret).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.json);
    assert!(a.json["token"].is_string());
    let next = secret_of(&a.cookie.expect("a fresh secret"));
    assert_ne!(next, secret);
}

const ON: RememberConfig = RememberConfig {
    on: true,
    idle_secs: 30 * 24 * 3600,
};

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_sets_a_renewal_cookie_scripts_cannot_read() {
    let kept = Kept::new();
    let app = kept.at(1_000, ON);
    let (_, cookie) = sign_in_from(&app, "ordo.example.com").await;
    let cookie = cookie.expect("a renewal cookie");
    assert!(cookie.starts_with("__Secure-ordo_renew="), "{cookie}");
    for flag in [
        "HttpOnly",
        "SameSite=Strict",
        "Path=/auth/refresh",
        "Max-Age=2592000",
        "Secure",
    ] {
        assert!(cookie.contains(flag), "{flag} missing from {cookie}");
    }
    // Plain http on loopback: a Secure cookie would be dropped there.
    let (_, local) = sign_in_from(&app, "127.0.0.1:4000").await;
    let local = local.expect("a renewal cookie");
    assert!(!local.contains("Secure"), "{local}");
    assert!(local.contains("HttpOnly") && local.contains("SameSite=Strict"));
    assert_eq!(kept.renewals.len(), 2, "one stored hash per sign-in");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reload_resumes_the_session_and_rotates_the_cookie() {
    let kept = Kept::new();
    let (first, cookie) = sign_in_from(&kept.at(1_000, ON), "127.0.0.1:4000").await;
    let secret = secret_of(&cookie.expect("cookie"));

    // A day later the browser is reopened.
    let a = reload(&kept.at(1_000 + 86_400, ON), &secret).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.json);
    assert_eq!(a.json["resumed"], true);
    let token = a.json["token"].as_str().expect("a token");
    let claims = ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        token,
        ordo_identity::Timestamp(1_000 + 86_400),
    )
    .expect("the resumed token verifies");
    let original = ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        &first,
        ordo_identity::Timestamp(1_000),
    )
    .expect("the first token verifies");
    assert_eq!(claims.principal, original.principal);
    assert_eq!(
        claims.session.as_ref().map(|s| &s.id),
        original.session.as_ref().map(|s| &s.id),
        "the same family, so a sign-out of either ends both"
    );
    assert!(claims.user_presence_at.is_none(), "nobody touched anything");
    let next = secret_of(&a.cookie.expect("a new cookie"));
    assert_ne!(next, secret, "each use sets a new secret");

    // The new one works on the next reload.
    let a = reload(&kept.at(1_000 + 2 * 86_400, ON), &next).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.json);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_tabs_loading_together_are_not_reuse() {
    let kept = Kept::new();
    let (_, cookie) = sign_in_from(&kept.at(1_000, ON), "127.0.0.1:4000").await;
    let secret = secret_of(&cookie.expect("cookie"));
    let a = reload(&kept.at(2_000, ON), &secret).await;
    assert_eq!(a.status, StatusCode::OK);
    // The second tab sent the same cookie a few seconds later.
    let b = reload(&kept.at(2_005, ON), &secret).await;
    assert_eq!(b.status, StatusCode::OK, "{}", b.json);
    assert!(b.cookie.is_none(), "no second secret is handed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn reusing_a_spent_cookie_ends_the_whole_family() {
    let kept = Kept::new();
    let (_, cookie) = sign_in_from(&kept.at(1_000, ON), "127.0.0.1:4000").await;
    let stolen = secret_of(&cookie.expect("cookie"));
    // The person reloads; their cookie turns.
    let a = reload(&kept.at(2_000, ON), &stolen).await;
    let theirs = secret_of(&a.cookie.expect("cookie"));
    let token = a.json["token"].as_str().expect("token").to_string();

    // A copy of the old cookie turns up an hour later.
    let r = reload(&kept.at(5_600, ON), &stolen).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.json["error"]["code"], "authentication_failed");
    assert!(r.cookie.expect("cleared").contains("Max-Age=0"));

    // The person's own cookie is ended too, and so is the session.
    let r = reload(&kept.at(5_601, ON), &theirs).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let (status, _) = post_bearer(&kept.at(5_602, ON), "/auth/refresh", &token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the session was revoked");
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_out_ends_the_renewal() {
    let kept = Kept::new();
    let app = kept.at(1_000, ON);
    let (token, cookie) = sign_in_from(&app, "127.0.0.1:4000").await;
    let secret = secret_of(&cookie.expect("cookie"));
    let out = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/auth/logout")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(out.status, StatusCode::NO_CONTENT);
    assert!(out
        .cookie
        .expect("the cookie is cleared")
        .contains("Max-Age=0"));
    let r = reload(&kept.at(1_100, ON), &secret).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.json);
    assert!(kept.renewals.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_out_a_resumed_token_ends_the_renewal() {
    let kept = Kept::new();
    let (_, cookie) = sign_in_from(&kept.at(1_000, ON), "127.0.0.1:4000").await;
    let a = reload(&kept.at(2_000, ON), &secret_of(&cookie.expect("cookie"))).await;
    let next = secret_of(&a.cookie.expect("cookie"));
    let token = a.json["token"].as_str().expect("token");
    let (status, _) = post_bearer(&kept.at(2_100, ON), "/auth/logout", token).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let r = reload(&kept.at(2_200, ON), &next).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_every_session_ends_every_renewal() {
    let kept = Kept::new();
    let app = kept.at(1_000, ON);
    let (phone, _) = sign_in_from(&app, "127.0.0.1:4000").await;
    let (_, laptop) = sign_in_from(&app, "127.0.0.1:4000").await;
    let laptop = secret_of(&laptop.expect("cookie"));
    let (status, _) = post_bearer(&kept.at(1_500, ON), "/auth/logout-all", &phone).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let r = reload(&kept.at(1_600, ON), &laptop).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // Long after the epoch row could be swept, it stays ended.
    let r = reload(&kept.at(1_000 + 20 * 86_400, ON), &laptop).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(kept.renewals.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_renewal_unused_past_its_idle_lifetime_ends() {
    let kept = Kept::new();
    let short = RememberConfig {
        on: true,
        idle_secs: 600,
    };
    let (_, cookie) = sign_in_from(&kept.at(1_000, short), "127.0.0.1:4000").await;
    let secret = secret_of(&cookie.expect("cookie"));
    // Used inside the window, it is renewed from the moment of use.
    let a = reload(&kept.at(1_500, short), &secret).await;
    assert_eq!(a.status, StatusCode::OK);
    let next = secret_of(&a.cookie.expect("cookie"));
    // 600 s idle after that use is too long.
    let r = reload(&kept.at(2_100, short), &next).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.json);
}

#[tokio::test(flavor = "multi_thread")]
async fn switched_off_no_cookie_is_set_and_the_refusal_names_the_setting() {
    let kept = Kept::new();
    let off = RememberConfig {
        on: false,
        idle_secs: 600,
    };
    let (_, cookie) = sign_in_from(&kept.at(1_000, off), "127.0.0.1:4000").await;
    assert!(cookie.is_none(), "off sets no cookie");
    let r = reload(&kept.at(1_100, off), "anything").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.json["error"]["code"], "remember_off");
    let message = r.json["error"]["message"].as_str().expect("message");
    assert!(message.contains("ORDO_REMEMBER=on"), "{message}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_page_may_ask_not_to_be_remembered() {
    let kept = Kept::new();
    let (_, cookie) = sign_in_with(&kept.at(1_000, ON), "127.0.0.1:4000", Some(false)).await;
    assert!(cookie.is_none());
    assert!(kept.renewals.is_empty());
}

// ── Session settings in three layers (owner, 2026-10-21) ────────────────────

use ordo_host::session_policy::{Deployment, SessionPolicies, READ_TENANT, SET_APP, SET_TENANT};

fn bound(pairs: &[(&str, &str)]) -> ordo_api::BoundParams {
    ordo_api::BoundParams(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect(),
    )
}

/// The routes at `at`, with the policy layers in memory.
fn with_policy(kept: &Kept, at: u64, policy: Arc<SessionPolicies>) -> axum::Router {
    let provider = LocalPasswordProvider::new(Argon2idHasher::weak_for_tests());
    provider
        .enroll("ana@example.test", "correct-horse")
        .expect("enrolled");
    let mut registry = ProviderRegistry::new();
    registry
        .register("password", Box::new(provider))
        .expect("one provider");
    let mut state = AuthState::new(
        registry,
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(RenewingRevocations::new(
        kept.revocations.clone(),
        kept.renewals.clone(),
    )))
    .with_remember(ordo_host::renewal::Remember {
        store: kept.renewals.clone(),
        config: ON,
        app_origins: app_origins(),
    })
    .with_policy(policy)
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()));
    state.now = Box::new(move || at);
    ordo_host::auth::router(Some(Arc::new(state)))
}

/// Sign in to `space`; the whole answer.
async fn sign_in_to(app: &axum::Router, space: &str) -> Answer {
    let (_, begun) = post(
        app,
        "/auth/begin",
        serde_json::json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let body = serde_json::json!({
        "method": "password", "kind": "password",
        "challenge": begun["challenge"].as_str().expect("a challenge"),
        "identifier": "ana@example.test", "password": "correct-horse",
        "space": space,
    });
    send(
        app,
        Request::builder()
            .method("POST")
            .uri("/auth/complete")
            .header("host", "127.0.0.1:4000")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request"),
    )
    .await
}

fn policies() -> Arc<SessionPolicies> {
    Arc::new(SessionPolicies::in_memory(Deployment::with_defaults(
        3_600, 43_200, ON,
    )))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_shorter_than_the_app_default_takes_effect() {
    let p = policies();
    p.execute(SET_APP, "system", &bound(&[("tokenSecs", "1800")]), "dev")
        .await
        .expect("the app developer's default");
    p.execute(
        SET_TENANT,
        "spc_short",
        &bound(&[("tokenSecs", "600"), ("maxSecs", "3600")]),
        "owner",
    )
    .await
    .expect("the tenant's own");
    let kept = Kept::new();
    // Signed in with no tenant: the app developer's default.
    let a = sign_in_to(&with_policy(&kept, 1_000, p.clone()), "").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.json);
    assert_eq!(a.json["expires_at"], 1_000 + 1_800, "{}", a.json);
    let token = a.json["token"].as_str().expect("token").to_string();
    // The session is for the tenant (a sign-in records this when the person
    // arrives there; admission needs a database, so it is planted here).
    let claims = ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        &token,
        ordo_identity::Timestamp(1_000),
    )
    .expect("verifies");
    let id = claims.session.expect("a session").id.0;
    p.note_session(&id, "spc_short", 1_000 + 43_200).await;

    // A refresh resolves the tenant's values, shorter than the app's.
    let (status, body) = post_bearer(
        &with_policy(&kept, 1_500, p.clone()),
        "/auth/refresh",
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["expires_at"], 1_500 + 600);
    assert_eq!(body["session_expires_at"], 1_000 + 3_600);
    let next = body["token"].as_str().expect("token").to_string();
    assert!(
        ordo_identity::verify(
            &TokenKey::from_secret(SECRET),
            &next,
            ordo_identity::Timestamp(2_100),
        )
        .is_err(),
        "the token itself carries the tenant's life"
    );
    // Past the tenant's ceiling, the session is over.
    let (status, _) = post_bearer(&with_policy(&kept, 4_700, p), "/auth/refresh", &next).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn no_tenant_value_falls_back_to_the_app_developer_then_the_deployment() {
    let p = policies();
    p.execute(SET_APP, "system", &bound(&[("tokenSecs", "1800")]), "dev")
        .await
        .expect("the app developer's default");
    let kept = Kept::new();
    let a = sign_in_to(&with_policy(&kept, 1_000, p.clone()), "spc_plain").await;
    assert_eq!(a.json["expires_at"], 1_000 + 1_800, "{}", a.json);
    assert_eq!(
        a.json["session_expires_at"],
        1_000 + 43_200,
        "the deployment's ceiling"
    );
    let read = p
        .execute(READ_TENANT, "spc_plain", &bound(&[]), "owner")
        .await
        .expect("the read");
    let row = &read.rows[0];
    assert_eq!(row["tokenSecs"], 1_800);
    assert_eq!(row["tokenSecsSource"], "app");
    assert_eq!(row["maxSecsSource"], "deployment");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_value_outside_the_range_is_refused_naming_the_range_and_who_sets_it() {
    let p = policies();
    p.execute(
        SET_APP,
        "system",
        &bound(&[("tokenSecsRange", "300-1800")]),
        "dev",
    )
    .await
    .expect("the app developer's range");
    let e = p
        .execute(
            SET_TENANT,
            "spc_t",
            &bound(&[("tokenSecs", "7200")]),
            "owner",
        )
        .await
        .expect_err("outside the range");
    assert!(e.message.contains("300-1800"), "{}", e.message);
    assert!(e.message.contains("app developer"), "{}", e.message);
    assert!(
        e.message.contains("std.deployment.setSessionPolicy"),
        "{}",
        e.message
    );

    // With no range, the deployment's limit, named by its setting. A tenant
    // may choose longer than the app developer's default inside it.
    let p = policies();
    let ok = p
        .execute(
            SET_TENANT,
            "spc_t",
            &bound(&[("tokenSecs", "7200")]),
            "owner",
        )
        .await
        .expect("longer than the default, inside the limit");
    assert_eq!(ok.rows[0]["tokenSecs"], 7_200);
    let e = p
        .execute(
            SET_TENANT,
            "spc_t",
            &bound(&[("tokenSecs", "999999")]),
            "owner",
        )
        .await
        .expect_err("past the deployment's limit");
    assert!(
        e.message.contains("ORDO_SESSION_SECS_LIMIT"),
        "{}",
        e.message
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_that_turns_renewals_off_ends_them_at_the_next_reload() {
    let p = policies();
    let kept = Kept::new();
    let a = sign_in_to(&with_policy(&kept, 1_000, p.clone()), "").await;
    assert!(a.cookie.is_some(), "a cookie while on");
    // A renewal for a sign-in into the tenant (planted: admission needs a
    // database).
    use ordo_host::renewal::RenewalStore;
    kept.renewals
        .issue(
            &ordo_host::renewal::Family {
                family: "sess_strict".into(),
                principal: "prn_ana".into(),
                via: Some("password".into()),
                second_step: None,
                space: Some("spc_strict".into()),
                signed_in_at: 1_000,
            },
            &ordo_host::renewal::hash_of("strict-secret"),
            1_000 + 86_400,
        )
        .expect("issued");
    let secret = "strict-secret".to_string();
    p.execute(
        SET_TENANT,
        "spc_strict",
        &bound(&[("remember", "off")]),
        "owner",
    )
    .await
    .expect("off for this tenant");
    let r = reload(&with_policy(&kept, 1_100, p.clone()), &secret).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.json["error"]["code"], "remember_off");
    assert!(r.json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("std.space.setSessionPolicy"));
    // Before it was turned off, the same cookie worked.
    let p2 = policies();
    let kept2 = Kept::new();
    kept2
        .renewals
        .issue(
            &ordo_host::renewal::Family {
                family: "sess_open".into(),
                principal: "prn_ana".into(),
                via: Some("password".into()),
                second_step: None,
                space: Some("spc_strict".into()),
                signed_in_at: 1_000,
            },
            &ordo_host::renewal::hash_of("open-secret"),
            1_000 + 86_400,
        )
        .expect("issued");
    let r = reload(&with_policy(&kept2, 1_100, p2), "open-secret").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(r.json["space"], "spc_strict");
}

// ── The overall limit on a remembered sign-in (owner, 2026-10-22) ──────────

/// A renewal for `space`, planted as a sign-in at `at` (admission needs a
/// database). Returns the secret.
fn plant_family(kept: &Kept, family: &str, space: Option<&str>, at: u64) -> String {
    use ordo_host::renewal::RenewalStore;
    let secret = format!("{family}-secret");
    kept.renewals
        .issue(
            &ordo_host::renewal::Family {
                family: family.into(),
                principal: "prn_ana".into(),
                via: Some("password".into()),
                second_step: None,
                space: space.map(str::to_string),
                signed_in_at: at,
            },
            &ordo_host::renewal::hash_of(&secret),
            at + 30 * 86_400,
        )
        .expect("issued");
    secret
}

/// **N seconds after the sign-in, sign in again, however often it was
/// used.** Daily use keeps the idle lifetime fresh; the overall limit does
/// not move.
#[tokio::test(flavor = "multi_thread")]
async fn the_overall_limit_ends_a_sign_in_despite_use() {
    let p = policies();
    p.execute(
        SET_APP,
        "system",
        &bound(&[("rememberMaxSecs", "259200")]),
        "dev",
    )
    .await
    .expect("three days overall");
    let kept = Kept::new();
    let a = sign_in_to(&with_policy(&kept, 1_000, p.clone()), "").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.json);
    let cookie = a.cookie.expect("a cookie");
    assert!(
        cookie.contains("Max-Age=259200"),
        "the cookie never outlives the overall limit: {cookie}"
    );
    let mut secret = secret_of(&cookie);
    // Used every day: each reload works and rotates the cookie.
    for day in 1..3u64 {
        let at = 1_000 + day * 86_400;
        let r = reload(&with_policy(&kept, at, p.clone()), &secret).await;
        assert_eq!(r.status, StatusCode::OK, "day {day}: {}", r.json);
        assert!(
            r.json["session_expires_at"].as_u64().expect("a ceiling") <= 1_000 + 259_200,
            "a resumed session never passes the overall end: {}",
            r.json
        );
        secret = secret_of(&r.cookie.expect("a new cookie"));
    }
    // Three days after the sign-in, the same busy device signs in again.
    let r = reload(&with_policy(&kept, 1_000 + 259_200, p.clone()), &secret).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.json);
    assert!(
        r.cookie.as_deref().is_some_and(|c| c.contains("Max-Age=0")),
        "the cookie is cleared"
    );
}

/// A tenant's overall limit shorter than the app developer's wins for
/// sign-ins into that tenant, and no other.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_overall_limit_shorter_than_the_app_wins() {
    let p = policies();
    p.execute(
        SET_APP,
        "system",
        &bound(&[("rememberMaxSecs", "604800")]),
        "dev",
    )
    .await
    .expect("a week overall");
    p.execute(
        SET_TENANT,
        "spc_strict",
        &bound(&[("rememberMaxSecs", "86400")]),
        "owner",
    )
    .await
    .expect("a day for this tenant");
    let read = p
        .execute(READ_TENANT, "spc_strict", &bound(&[]), "owner")
        .await
        .expect("the read");
    assert_eq!(read.rows[0]["rememberMaxSecs"], 86_400);
    assert_eq!(read.rows[0]["rememberMaxSecsSource"], "tenant");
    let kept = Kept::new();
    let strict = plant_family(&kept, "sess_strict", Some("spc_strict"), 1_000);
    let plain = plant_family(&kept, "sess_plain", Some("spc_plain"), 1_000);
    let at = 1_000 + 86_400;
    let r = reload(&with_policy(&kept, at, p.clone()), &strict).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.json);
    let r = reload(&with_policy(&kept, at, p.clone()), &plain).await;
    assert_eq!(r.status, StatusCode::OK, "the app's week: {}", r.json);
}

/// With nothing set there is no overall limit: a device used often enough
/// stays signed in.
#[tokio::test(flavor = "multi_thread")]
async fn there_is_no_overall_limit_by_default() {
    let p = policies();
    let read = p
        .execute(READ_TENANT, "spc_any", &bound(&[]), "owner")
        .await
        .expect("the read");
    assert_eq!(read.rows[0]["rememberMaxSecs"], "none");
    let kept = Kept::new();
    let a = sign_in_to(&with_policy(&kept, 1_000, p.clone()), "").await;
    let mut secret = secret_of(&a.cookie.expect("a cookie"));
    // Every 20 days for 200 days.
    for n in 1..=10u64 {
        let at = 1_000 + n * 20 * 86_400;
        let r = reload(&with_policy(&kept, at, p.clone()), &secret).await;
        assert_eq!(r.status, StatusCode::OK, "reload {n}: {}", r.json);
        secret = secret_of(&r.cookie.expect("a new cookie"));
    }
}
