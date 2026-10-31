//! One sign-in across subdomains (`plan-subdomain-signin.md`, owner rulings
//! 2026-10-09): the core site hands a signed-in person to a subdomain with a
//! one-time code bound to a binder, and the subdomain gets its own session.
//!
//! What it proves:
//!
//! - the round trip gives the subdomain a session for the same person, with
//!   the same ceiling, no presence, and `via` naming the hand-off, and the
//!   door accepts it;
//! - a code is refused when reused, expired, offered with the wrong binder,
//!   or redeemed from another origin;
//! - a return on an allowed origin is accepted on any path, and one on any
//!   other origin is refused, with the setting named in the stream;
//! - a wildcard return is refused while `ORDO_WILDCARD_ORIGINS` is off,
//!   matches one label when on, and never matches `evil.com` or
//!   `x.example.com.evil.com`;
//! - a session that ended cannot issue a code, and a code outlives no
//!   session that ended after issuing it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{AccessChecker, AccessDecision};
use ordo_host::auth::AuthState;
use ordo_host::metrics::Metrics;
use ordo_host::IdentityAccess;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::provider::{Argon2idHasher, LocalPasswordProvider};
use ordo_identity::{
    AccessEngine, AccessProfile, MemoryCeremonyStore, MemorySessionRevocations, ProviderRegistry,
    Role, SpaceId, Timestamp, TokenKey,
};
use ordo_secevent::EventKind;
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &[u8] = b"auth-handoff-test-secret";
const NOW: u64 = 1_800_000_000;
const APP: &str = "https://app.example.com";
const SPACE: &str = "spc_app";
const PLAIN: &str = "posts.list";

struct H {
    auth: Arc<AuthState>,
    app: axum::Router,
    revocations: Arc<MemorySessionRevocations>,
    clock: Arc<AtomicU64>,
}

fn harness(returns: Option<&str>, wildcards: bool) -> H {
    let provider = LocalPasswordProvider::new(Argon2idHasher::weak_for_tests());
    provider
        .enroll("ana@example.test", "correct-horse")
        .expect("enrolled");
    let mut registry = ProviderRegistry::new();
    registry
        .register("password", Box::new(provider))
        .expect("one provider");
    let revocations = Arc::new(MemorySessionRevocations::new());
    let returns = returns
        .map(|r| ordo_host::federation::parse_return_urls(r, wildcards).expect("a return list"));
    let mut state = AuthState::new(
        registry,
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations.clone())
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()))
    .with_ceremonies(Arc::new(MemoryCeremonyStore::new()))
    .with_handoff(returns, 60);
    let clock = Arc::new(AtomicU64::new(NOW));
    let c = clock.clone();
    state.now = Box::new(move || c.load(Ordering::SeqCst));
    let auth = Arc::new(state);
    H {
        app: ordo_host::auth::router(Some(auth.clone())),
        auth,
        revocations,
        clock,
    }
}

async fn call(
    app: &axum::Router,
    path: &str,
    bearer: Option<&str>,
    origin: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    if let Some(o) = origin {
        req = req.header("origin", o);
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).expect("request"))
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

/// The core site's sign-in: a password, here. Any method would do.
async fn core_sign_in(h: &H) -> Value {
    let (_, begun) = call(
        &h.app,
        "/auth/begin",
        None,
        None,
        json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let (status, done) = call(
        &h.app,
        "/auth/complete",
        None,
        None,
        json!({
            "method": "password", "kind": "password",
            "challenge": begun["challenge"], "identifier": "ana@example.test",
            "password": "correct-horse", "display": "Ana"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done
}

/// What the subdomain page makes and keeps: a binder, and its public half.
fn binder(seed: &str) -> (String, String) {
    let b = format!("binder-{seed}-0123456789abcdefghijklmnopqrstuvwxyz");
    let h = ordo_identity::federation::handle_of(&b);
    (b, h)
}

async fn issue(h: &H, token: &str, return_to: &str, binder_hash: &str) -> (StatusCode, Value) {
    call(
        &h.app,
        "/auth/handoff",
        Some(token),
        None,
        json!({"return_to": return_to, "binder_hash": binder_hash}),
    )
    .await
}

fn code_of(redirect: &str) -> String {
    redirect
        .split_once("handoff_code=")
        .expect("a code in the redirect")
        .1
        .to_string()
}

async fn redeem(h: &H, code: &str, binder: &str, origin: Option<&str>) -> (StatusCode, Value) {
    call(
        &h.app,
        "/auth/handoff/redeem",
        None,
        origin,
        json!({"code": code, "binder": binder}),
    )
    .await
}

fn claims(token: &str, at: u64) -> ordo_identity::TokenClaims {
    ordo_identity::verify(&TokenKey::from_secret(SECRET), token, Timestamp(at)).expect("verifies")
}

fn events(h: &H) -> Vec<EventKind> {
    h.auth
        .recorder
        .as_ref()
        .expect("recorder")
        .drain_all(ordo_commandlog::Timestamp(NOW + 100_000))
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

fn reasons(h: &H) -> Vec<String> {
    events(h)
        .into_iter()
        .filter_map(|k| match k {
            EventKind::AuthenticationFailed { reason } => Some(reason),
            _ => None,
        })
        .collect()
}

fn refused(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "authentication_failed", "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hand_off_gives_the_subdomain_its_own_session_for_the_same_person() {
    let h = harness(Some("https://app.example.com/signed-in"), false);
    let core = core_sign_in(&h).await;
    let core_token = core["token"].as_str().expect("a token");
    let (b, hash) = binder("round-trip");
    // A different path on the listed origin: the person lands where they were.
    let (status, issued) = issue(
        &h,
        core_token,
        "https://app.example.com/notes/7?tab=a",
        &hash,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    let redirect = issued["redirect"].as_str().expect("a redirect");
    assert!(
        redirect.starts_with("https://app.example.com/notes/7?tab=a&handoff_code="),
        "{redirect}"
    );
    assert_eq!(issued["expires_at"], NOW + 60);

    let (status, done) = redeem(&h, &code_of(redirect), &b, Some(APP)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["via"], "handoff");
    let token = done["token"].as_str().expect("a session");
    assert_ne!(token, core_token);

    let core_claims = claims(core_token, NOW);
    let sub = claims(token, NOW);
    assert_eq!(sub.principal, core_claims.principal, "the same person");
    assert_eq!(sub.user_presence_at, None, "a hand-off carries no presence");
    let (cs, ss) = (
        core_claims.session.as_ref().expect("a session"),
        sub.session.as_ref().expect("a session"),
    );
    assert_ne!(ss.id, cs.id, "its own session");
    assert_eq!(ss.started_at, cs.started_at, "the same ceiling");
    assert_eq!(ss.via.as_deref(), Some("handoff"));
    assert_eq!(done["session_expires_at"], core["session_expires_at"]);

    // The door accepts it.
    let space = SpaceId(SPACE.into());
    let mut engine = AccessEngine::new();
    engine.set_profile(&space, AccessProfile::new().allow(Role::Owner, PLAIN));
    engine.set_membership(&space, &sub.principal, Role::Owner);
    let door = IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW + 1),
        Arc::new(Metrics::new()),
    )
    .with_revocations(h.revocations.clone());
    assert_eq!(
        door.check(token, SPACE, PLAIN),
        AccessDecision::Allow {
            principal: sub.principal.0.clone(),
            agent: None,
        }
    );

    // Issuing and redeeming are both in the stream, and the code is in
    // neither.
    let code = code_of(redirect);
    let acts: Vec<String> = events(&h)
        .into_iter()
        .filter_map(|k| match k {
            EventKind::SessionHandoff { act, detail } => {
                assert!(!detail.contains(&code), "{detail}");
                assert!(
                    detail.contains("origin=https://app.example.com"),
                    "{detail}"
                );
                Some(act)
            }
            _ => None,
        })
        .collect();
    assert_eq!(acts, ["issued", "redeemed"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_code_is_used_once() {
    let h = harness(Some(APP), false);
    let core = core_sign_in(&h).await;
    let (b, hash) = binder("once");
    let (_, issued) = issue(&h, core["token"].as_str().unwrap(), APP, &hash).await;
    let code = code_of(issued["redirect"].as_str().unwrap());
    let (status, _) = redeem(&h, &code, &b, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, again) = redeem(&h, &code, &b, None).await;
    refused(status, &again);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_code_is_refused() {
    let h = harness(Some(APP), false);
    let core = core_sign_in(&h).await;
    let (b, hash) = binder("late");
    let (_, issued) = issue(&h, core["token"].as_str().unwrap(), APP, &hash).await;
    let code = code_of(issued["redirect"].as_str().unwrap());
    h.clock.store(NOW + 60, Ordering::SeqCst);
    let (status, late) = redeem(&h, &code, &b, None).await;
    refused(status, &late);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_binder_is_refused_and_does_not_burn_the_code() {
    let h = harness(Some(APP), false);
    let core = core_sign_in(&h).await;
    let (b, hash) = binder("mine");
    let (thief, _) = binder("thief");
    let (_, issued) = issue(&h, core["token"].as_str().unwrap(), APP, &hash).await;
    let code = code_of(issued["redirect"].as_str().unwrap());
    let (status, stolen) = redeem(&h, &code, &thief, None).await;
    refused(status, &stolen);
    let (status, mine) = redeem(&h, &code, &b, None).await;
    assert_eq!(status, StatusCode::OK, "{mine}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_code_redeemed_from_another_origin_is_refused() {
    let h = harness(Some(APP), false);
    let core = core_sign_in(&h).await;
    let (b, hash) = binder("origin");
    let (_, issued) = issue(&h, core["token"].as_str().unwrap(), APP, &hash).await;
    let code = code_of(issued["redirect"].as_str().unwrap());
    let (status, body) = redeem(&h, &code, &b, Some("https://evil.com")).await;
    refused(status, &body);
    assert!(reasons(&h).iter().any(|r| r == "origin-mismatch"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_return_on_another_origin_is_refused_and_names_the_setting() {
    let h = harness(Some("https://app.example.com/signed-in"), false);
    let core = core_sign_in(&h).await;
    let token = core["token"].as_str().unwrap();
    let (_, hash) = binder("where");
    for bad in [
        "https://evil.com/signed-in",
        "https://app.example.com.evil.com/signed-in",
        "https://app.example.com@evil.com/",
        "https://other.example.com/signed-in",
        "http://app.example.com/signed-in",
        "https://app.example.com:8443/signed-in",
        "https://app.example.com/signed-in#frag",
    ] {
        let (status, body) = issue(&h, token, bad, &hash).await;
        refused(status, &body);
    }
    let named = reasons(&h);
    assert!(
        named
            .iter()
            .all(|r| r.starts_with("return-not-allowed") && r.contains("ORDO_AUTH_RETURN_URLS")),
        "{named:?}"
    );
    assert_eq!(named.len(), 7);
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_return_list_every_hand_off_is_refused_and_says_which_setting() {
    let h = harness(None, false);
    let core = core_sign_in(&h).await;
    let (_, hash) = binder("unset");
    let (status, body) = issue(&h, core["token"].as_str().unwrap(), APP, &hash).await;
    refused(status, &body);
    assert!(reasons(&h)
        .iter()
        .any(|r| r.starts_with("returns-unset") && r.contains("ORDO_AUTH_RETURN_URLS")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wildcard_return_needs_the_setting_and_matches_one_label_only() {
    // Off: the list itself is refused, by name, so the boot is.
    let off = ordo_host::federation::parse_return_urls("https://*.example.com", false)
        .expect_err("a wildcard with the setting off");
    assert!(off.contains("ORDO_WILDCARD_ORIGINS=on"), "{off}");
    let exact_only = harness(Some(APP), false);
    let core = core_sign_in(&exact_only).await;
    let (_, hash) = binder("wild-off");
    let (status, body) = issue(
        &exact_only,
        core["token"].as_str().unwrap(),
        "https://x.example.com/",
        &hash,
    )
    .await;
    refused(status, &body);

    // On: one label under the domain, and nothing else.
    let h = harness(Some("https://*.example.com"), true);
    let core = core_sign_in(&h).await;
    let token = core["token"].as_str().unwrap();
    let (b, hash) = binder("wild-on");
    let (status, issued) = issue(&h, token, "https://x.example.com/any/path", &hash).await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    let code = code_of(issued["redirect"].as_str().unwrap());
    let (status, done) = redeem(&h, &code, &b, Some("https://x.example.com")).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    for bad in [
        "https://evil.com/",
        "https://x.example.com.evil.com/",
        "https://example.com/",
        "https://a.b.example.com/",
        "http://x.example.com/",
    ] {
        let (status, body) = issue(&h, token, bad, &hash).await;
        refused(status, &body);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ended_session_cannot_issue_and_its_codes_die_with_it() {
    let h = harness(Some(APP), false);
    let core = core_sign_in(&h).await;
    let token = core["token"].as_str().unwrap().to_string();
    let (b, hash) = binder("ended");
    let (_, issued) = issue(&h, &token, APP, &hash).await;
    let code = code_of(issued["redirect"].as_str().unwrap());
    let (status, _) = call(&h.app, "/auth/logout", Some(&token), None, json!({})).await;
    assert!(status.is_success(), "{status}");
    let (status, body) = redeem(&h, &code, &b, None).await;
    refused(status, &body);
    let (status, body) = issue(&h, &token, APP, &hash).await;
    refused(status, &body);
    let named = reasons(&h);
    assert!(
        named.iter().any(|r| r == "issuing-session-ended"),
        "{named:?}"
    );
    assert!(named.iter().any(|r| r == "session-ended"), "{named:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hand_off_needs_a_session_token_and_a_plausible_binder_hash() {
    let h = harness(Some(APP), false);
    let core = core_sign_in(&h).await;
    let (status, body) = call(
        &h.app,
        "/auth/handoff",
        None,
        None,
        json!({"return_to": APP, "binder_hash": binder("x").1}),
    )
    .await;
    refused(status, &body);
    let (status, body) = issue(&h, core["token"].as_str().unwrap(), APP, "short").await;
    refused(status, &body);
    let (status, body) = issue(&h, "not-a-token", APP, &binder("y").1).await;
    refused(status, &body);
}
