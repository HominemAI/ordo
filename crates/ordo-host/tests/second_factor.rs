//! The second factor through the real routes (`plan-second-factor.md`
//! T1 onward), with a software authenticator standing in for the person's
//! hands and a pinned clock standing in for the time between attempts.
//!
//! Nothing fakes a gesture. `TestAuthenticator` holds a real key and emits the
//! bytes a real authenticator emits, and the routes verify them as they would
//! verify a person's.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_host::auth::AuthState;
use ordo_host::second_factors::{SecondFactorConfig, SecondFactors, SecondStepMethod, Verdict};
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::provider::{Argon2idHasher, LocalPasswordProvider};
use ordo_identity::{
    MemorySessionRevocations, PasskeyProvider, ProviderRegistry, RelyingParty, SecondStepKind,
    TokenKey,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &[u8] = b"second-factor-test-secret";
const NOW: u64 = 1_000_000;

/// A method with one fixed answer, so the flow can be tested apart from any
/// real way of checking.
struct Fixed {
    kind: SecondStepKind,
    code: &'static str,
    held: bool,
}

impl SecondStepMethod for Fixed {
    fn kind(&self) -> SecondStepKind {
        self.kind
    }
    fn holds(&self, _principal: &str) -> Result<bool, ordo_identity::StoreFault> {
        Ok(self.held)
    }
    fn verify(
        &self,
        _principal: &str,
        body: &Value,
        _now: u64,
    ) -> Result<Verdict, ordo_identity::StoreFault> {
        Ok(if body["code"] == self.code {
            Verdict::Accepted
        } else {
            Verdict::Rejected { locked: false }
        })
    }
}

struct Harness {
    auth: Arc<AuthState>,
    clock: Arc<AtomicU64>,
    revocations: Arc<MemorySessionRevocations>,
}

impl Harness {
    fn set_now(&self, at: u64) {
        self.clock.store(at, Ordering::SeqCst);
    }
    fn app(&self) -> axum::Router {
        ordo_host::auth::router(Some(self.auth.clone()))
    }
}

/// A deployment with a password method for `ana@example.test`, a passkey
/// method, the given ceremony store, and the second step over `methods`.
fn harness_on(
    clock: Arc<AtomicU64>,
    ceremonies: Arc<dyn ordo_identity::CeremonyStore>,
    config: SecondFactorConfig,
    methods: Vec<Arc<dyn SecondStepMethod>>,
) -> Harness {
    let mut sf = SecondFactors::new(config);
    for m in methods {
        sf = sf.with_method(m);
    }
    harness_with(clock, ceremonies, sf)
}

/// The passkey provider a deployment holds, over memory key stores and the
/// given ceremonies.
fn passkey_over(ceremonies: Arc<dyn ordo_identity::CeremonyStore>) -> Arc<PasskeyProvider> {
    let keys = Arc::new(ordo_identity::MemoryPasskeyStore::new());
    let rp = RelyingParty::new("example.com", &["https://app.example.com"]).expect("rp");
    Arc::new(PasskeyProvider::with_stores(rp, keys, ceremonies))
}

fn harness_with(
    clock: Arc<AtomicU64>,
    ceremonies: Arc<dyn ordo_identity::CeremonyStore>,
    sf: SecondFactors,
) -> Harness {
    let passkey = passkey_over(ceremonies.clone());
    harness_passkey(clock, ceremonies, passkey, sf)
}

fn harness_passkey(
    clock: Arc<AtomicU64>,
    ceremonies: Arc<dyn ordo_identity::CeremonyStore>,
    passkey: Arc<PasskeyProvider>,
    sf: SecondFactors,
) -> Harness {
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
    let recorder = Arc::new(ordo_secevent::Recorder::default());
    let revocations = Arc::new(MemorySessionRevocations::new());
    let mut state = AuthState::new(
        registry,
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations.clone())
    .with_recorder(recorder)
    .with_ceremonies(ceremonies)
    .with_passkey(passkey);
    let read = clock.clone();
    state.now = Box::new(move || read.load(Ordering::SeqCst));
    let state = Arc::new(state);
    state.wire_second_factors(Arc::new(sf));
    Harness {
        auth: state,
        clock,
        revocations,
    }
}

fn harness(methods: Vec<Arc<dyn SecondStepMethod>>) -> Harness {
    harness_on(
        Arc::new(AtomicU64::new(NOW)),
        Arc::new(ordo_identity::MemoryCeremonyStore::new()),
        SecondFactorConfig::default(),
        methods,
    )
}

fn totp_stub() -> Arc<dyn SecondStepMethod> {
    Arc::new(Fixed {
        kind: SecondStepKind::Totp,
        code: "123456",
        held: true,
    })
}

async fn send(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    send_with(app, method, path, body, bearer, &[]).await
}

async fn send_with(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(path);
    if body.is_some() {
        b = b.header("content-type", "application/json");
    }
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    for (k, v) in headers {
        b = b.header(*k, *v);
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
    send(app, "POST", path, Some(body), None).await
}

/// The first step: a correct password. The whole answer.
async fn password_first_step(app: &axum::Router) -> (StatusCode, Value) {
    let (_, begun) = post(
        app,
        "/auth/begin",
        json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    post(
        app,
        "/auth/complete",
        json!({"method": "password", "kind": "password",
               "challenge": begun["challenge"], "identifier": "ana@example.test",
               "password": "correct-horse", "display": "Ana"}),
    )
    .await
}

fn claims_of(token: &str, at: u64) -> ordo_identity::TokenClaims {
    ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        token,
        ordo_identity::Timestamp(at),
    )
    .expect("a token this deployment minted")
}

fn drained(h: &Harness) -> Vec<ordo_secevent::StreamEntry> {
    h.auth
        .recorder
        .as_ref()
        .expect("recorder")
        .drain_all(ordo_commandlog::Timestamp(NOW + 1_000_000))
}

fn reasons(entries: &[ordo_secevent::StreamEntry]) -> Vec<String> {
    entries.iter().map(|e| e.render()).collect()
}

// ══════════════════════════ T1 · the half-signed-in state ═════════════════

/// **A password sign-in with a factor enrolled yields no token until the
/// second step.** The first step answers a handle and a binder and the
/// methods on offer. The second step mints, and the token names the factor.
/// A password proves no presence, and the second step adds none.
#[tokio::test(flavor = "multi_thread")]
async fn a_password_sign_in_with_a_factor_enrolled_yields_no_token_until_the_second_step() {
    let h = harness(vec![totp_stub()]);
    let app = h.app();
    let (status, half) = password_first_step(&app).await;
    assert_eq!(status, StatusCode::OK, "{half}");
    assert!(half.get("token").is_none(), "no token yet: {half}");
    assert_eq!(half["methods"], json!(["totp"]));
    let (handle, binder) = (half["second_step"].clone(), half["binder"].clone());
    assert!(handle.as_str().is_some_and(|s| s.starts_with("ss_")));
    assert_eq!(binder.as_str().map(str::len), Some(64), "256 bits");
    assert_eq!(half["expires_at"], NOW + 300);

    // A wrong code mints nothing, and is the one answer.
    let (status, wrong) = post(
        &app,
        "/auth/second-step",
        json!({"handle": handle, "binder": binder, "kind": "totp", "code": "000000"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{wrong}");
    assert_eq!(wrong["error"]["code"], "authentication_failed");
    assert!(wrong.get("token").is_none());

    // The right one does.
    let (status, done) = post(
        &app,
        "/auth/second-step",
        json!({"handle": handle, "binder": binder, "kind": "totp", "code": "123456"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["second_step"], "totp");
    let token = done["token"].as_str().expect("token");
    let claims = claims_of(token, NOW);
    assert_eq!(
        claims.session.as_ref().and_then(|s| s.second_step),
        Some(SecondStepKind::Totp)
    );
    assert!(
        claims.user_presence_at.is_none(),
        "a password and a second factor carry no presence"
    );

    // The handle was spent by the success.
    let (status, again) = post(
        &app,
        "/auth/second-step",
        json!({"handle": handle, "binder": binder, "kind": "totp", "code": "123456"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{again}");

    // The stream: the wrong answer with its reason, the wrong answer's
    // person is not named, and `Authenticated` is written once, at the end.
    let entries = drained(&h);
    let kinds = reasons(&entries);
    assert!(
        kinds.iter().any(|k| k.contains("reason=second-step-wrong")),
        "{kinds:?}"
    );
    let signed_in: u64 = entries
        .iter()
        .filter(|e| e.kind == ordo_secevent::EventKind::Authenticated)
        .map(|e| e.count)
        .sum();
    assert_eq!(signed_in, 1, "one sign-in, recorded when it completed");

    // Control: a person who holds no factor is signed in by the first step.
    let h = harness(vec![Arc::new(Fixed {
        kind: SecondStepKind::Totp,
        code: "123456",
        held: false,
    })]);
    let (status, signed) = password_first_step(&h.app()).await;
    assert_eq!(status, StatusCode::OK, "{signed}");
    assert!(signed["token"].as_str().is_some(), "{signed}");
    assert!(claims_of(signed["token"].as_str().unwrap(), NOW)
        .session
        .is_some_and(|s| s.second_step.is_none()));
}

/// **A handle without its binder is the one answer.** A handle seen in a log
/// is worthless alone, the wrong binder does not spend the person's attempts,
/// and the person who has both still completes.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_step_handle_without_its_binder_is_the_one_answer() {
    let h = harness(vec![totp_stub()]);
    let app = h.app();
    let (_, half) = password_first_step(&app).await;
    let (handle, binder) = (half["second_step"].clone(), half["binder"].clone());

    let (unknown_status, unknown) = post(
        &app,
        "/auth/second-step",
        json!({"handle": "ss_nothing", "binder": binder, "kind": "totp", "code": "123456"}),
    )
    .await;
    // Six wrong binders, more than the attempts a ceremony allows.
    for i in 0..6 {
        let (status, wrong) = post(
            &app,
            "/auth/second-step",
            json!({"handle": handle, "binder": format!("{i:064}"), "kind": "totp",
                   "code": "123456"}),
        )
        .await;
        assert_eq!(status, unknown_status);
        assert_eq!(wrong, unknown, "the same answer as a handle nobody minted");
    }
    let (status, _) = post(
        &app,
        "/auth/second-step",
        json!({"handle": handle, "kind": "totp", "code": "123456"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no binder at all");

    // The person, who holds the binder, is not locked out by any of that.
    let (status, done) = post(
        &app,
        "/auth/second-step",
        json!({"handle": handle, "binder": binder, "kind": "totp", "code": "123456"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let kinds = reasons(&drained(&h));
    assert!(
        kinds
            .iter()
            .any(|k| k.contains("reason=second-step-binder")),
        "the stream says why: {kinds:?}"
    );
}

/// A token from before the second-step claim existed is refused outright, at
/// the door that reads tokens and at the route that continues a session.
#[tokio::test(flavor = "multi_thread")]
async fn a_v3_token_is_refused_outright() {
    use sha2::{Digest, Sha256};
    fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
        let mut k = [0u8; 64];
        k[..key.len()].copy_from_slice(key);
        let mut inner = Sha256::new();
        inner.update(k.map(|b| b ^ 0x36));
        for p in parts {
            inner.update(p);
        }
        let mut outer = Sha256::new();
        outer.update(k.map(|b| b ^ 0x5c));
        outer.update(inner.finalize());
        outer.finalize().into()
    }
    fn b64(bytes: &[u8]) -> String {
        ordo_identity::webauthn::b64url_encode(bytes)
    }
    let key = hmac(b"ordo.token.key.v1", &[SECRET]);
    // The same key the deployment derives, over a payload in the old shape.
    let forge = |payload: &str| {
        let mac = hmac(
            &key,
            &[
                b"ordo.token.v1",
                &(payload.len() as u64).to_be_bytes(),
                payload.as_bytes(),
            ],
        );
        format!("{}.{}", b64(payload.as_bytes()), b64(&mac))
    };
    let now = NOW + 10;
    let old = forge(&format!("v3|usr_ana|{}|||||sess_old~{NOW}~", NOW + 3600));
    let current = forge(&format!("v6|usr_ana|{}|||||sess_new~{NOW}~~~~", NOW + 3600));
    let h = harness(vec![]);
    let app = h.app();
    h.set_now(now);
    let (status, _) = send(&app, "POST", "/auth/refresh", None, Some(&old)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a v3 token is refused");
    let (status, refreshed) = send(&app, "POST", "/auth/refresh", None, Some(&current)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the same token in the v4 shape is honoured: {refreshed}"
    );
}

/// **A second step begun on one process completes on another, and only
/// once.** Two replicas share one ceremony table; the same handle is answered
/// on both at once and exactly one of them mints.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_step_begun_on_one_process_completes_on_another_and_only_once() {
    let db = "ordo_secondfactor_replicas";
    let pg = ordo_conformance::Pg::named(db);
    let f = ordo_conformance::Fixture::load().expect("fixture");
    f.reset(&pg).expect("seeded");
    let cfg = ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 1,
        ..ordo_exec::PoolConfig::from_env()
    };
    let pool = ordo_exec::Pool::connect(cfg).await.expect("pool");
    ordo_host::ceremonies::ensure_table(&pool)
        .await
        .expect("ceremony table");
    let _ = pool.rows("DELETE FROM \"$authceremony\"").await;
    let clock = Arc::new(AtomicU64::new(NOW));
    let one = harness_on(
        clock.clone(),
        Arc::new(ordo_host::ceremonies::PgCeremonyStore::open_on(pool.clone()).expect("store")),
        SecondFactorConfig::default(),
        vec![totp_stub()],
    );
    let two = harness_on(
        clock.clone(),
        Arc::new(ordo_host::ceremonies::PgCeremonyStore::open_on(pool.clone()).expect("store")),
        SecondFactorConfig::default(),
        vec![totp_stub()],
    );
    let (status, half) = password_first_step(&one.app()).await;
    assert_eq!(status, StatusCode::OK, "{half}");
    let body = json!({"handle": half["second_step"], "binder": half["binder"],
                      "kind": "totp", "code": "123456"});
    let (app_a, app_b) = (one.app(), two.app());
    let (a, b) = tokio::join!(
        post(&app_a, "/auth/second-step", body.clone()),
        post(&app_b, "/auth/second-step", body.clone())
    );
    let won: Vec<&StatusCode> = [&a.0, &b.0]
        .into_iter()
        .filter(|s| **s == StatusCode::OK)
        .collect();
    assert_eq!(won.len(), 1, "one replica mints: {a:?} {b:?}");
    let (again, _) = post(&app_b, "/auth/second-step", body.clone()).await;
    assert_eq!(again, StatusCode::UNAUTHORIZED, "and only once");
}

// ══════════════════════════ T2 · code-generating tokens ═══════════════════

use ordo_host::second_factor_store::MemorySecondFactorStore;
use ordo_host::second_factors::{Sealer, TotpMethod};
use ordo_identity::totp;

/// RFC 4648 base32 back to bytes: what a person's app does with the secret.
fn unbase32(s: &str) -> Vec<u8> {
    const A: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut acc, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for c in s.bytes() {
        let v = A.iter().position(|a| *a == c).expect("base32") as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

struct Totp {
    h: Harness,
    store: Arc<MemorySecondFactorStore>,
}

/// A deployment whose second step is real code-generating factors, sealed under
/// a test key custody, over a memory store.
fn totp_harness(config: SecondFactorConfig) -> Totp {
    let store = Arc::new(MemorySecondFactorStore::new());
    let totp = Arc::new(TotpMethod::new(
        store.clone(),
        Sealer::new(Arc::new(ordo_crypto::TestProvider::new())),
        config.clone(),
    ));
    let sf = SecondFactors::new(config).with_totp(totp);
    let h = harness_with(
        Arc::new(AtomicU64::new(NOW)),
        Arc::new(ordo_identity::MemoryCeremonyStore::new()),
        sf,
    );
    Totp { h, store }
}

/// Sign in with the password, expecting a token: a person with no factor.
async fn token_by_password(app: &axum::Router) -> String {
    let (status, done) = password_first_step(app).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done["token"].as_str().expect("a token").to_string()
}

/// Start an enrolment with a session's bearer, giving the proof it asks for
/// when it asks: a session with fresh presence needs none, and a password
/// session is told to prove a first factor again and does (with the second step
/// behind it when the person holds a factor, answered with `secret`).
async fn begin_enrolment(
    app: &axum::Router,
    path: &str,
    token: &str,
    body: Value,
    secret: Option<&[u8]>,
    at: u64,
) -> (StatusCode, Value) {
    let (status, out) = send(app, "POST", path, Some(body.clone()), Some(token)).await;
    if status != StatusCode::FORBIDDEN {
        return (status, out);
    }
    assert_eq!(out["error"]["code"], "reauthentication_required", "{out}");
    let handle = reauth_handle(app, token, secret, at).await;
    send_with(
        app,
        "POST",
        path,
        Some(body),
        Some(token),
        &[("x-reauth", &handle)],
    )
    .await
}

/// Enrol a code-generating factor with a session's bearer: the secret the
/// person's app now holds.
async fn enrol_totp(app: &axum::Router, token: &str, now: u64) -> Vec<u8> {
    let (status, begun) = begin_enrolment(
        app,
        "/auth/factors/totp/begin",
        token,
        json!({"label": "phone"}),
        None,
        now,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let secret = unbase32(begun["secret"].as_str().expect("secret"));
    let code = totp::totp_at(&secret, now, totp::DIGITS);
    let (status, done) = send(
        app,
        "POST",
        "/auth/factors/totp/confirm",
        Some(json!({"factor": begun["factor"], "code": code})),
        Some(token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    secret
}

async fn answer_totp(app: &axum::Router, half: &Value, code: &str) -> (StatusCode, Value) {
    post(
        app,
        "/auth/second-step",
        json!({"handle": half["second_step"], "binder": half["binder"],
               "kind": "totp", "code": code}),
    )
    .await
}

/// **A code is accepted once and the same code is refused after.** The
/// enrolment proved the step it was made in, so that step's code cannot sign
/// in either. A later step signs in, and its code is spent for the rest of its
/// thirty seconds.
#[tokio::test(flavor = "multi_thread")]
async fn a_totp_code_is_accepted_once_and_the_same_code_is_refused_after() {
    let t = totp_harness(SecondFactorConfig::default());
    let app = t.h.app();
    let secret = enrol_totp(&app, &token_by_password(&app).await, NOW).await;

    // The secret is in the store sealed, and not as itself.
    let held = t.store.all();
    assert_eq!(held.len(), 1);
    assert!(!held[0].secret.contains(&totp::base32(&secret)));

    // The code the enrolment proved is spent.
    let (_, half) = password_first_step(&app).await;
    assert!(half.get("token").is_none(), "a factor is enrolled: {half}");
    let (status, _) = answer_totp(&app, &half, &totp::totp_at(&secret, NOW, 6)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the enrolment's own code");

    // A later step signs in.
    t.h.set_now(NOW + 30);
    let code = totp::totp_at(&secret, NOW + 30, 6);
    let (_, half) = password_first_step(&app).await;
    let (status, done) = answer_totp(&app, &half, &code).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["second_step"], "totp");

    // The same code, inside the same thirty seconds, on a fresh sign-in: refused.
    let (_, half) = password_first_step(&app).await;
    let (status, _) = answer_totp(&app, &half, &code).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a used code is spent");

    // The next step's code is good again.
    t.h.set_now(NOW + 61);
    let (_, half) = password_first_step(&app).await;
    let (status, _) = answer_totp(&app, &half, &totp::totp_at(&secret, NOW + 60, 6)).await;
    assert_eq!(status, StatusCode::OK);
}

/// **Five wrong codes end the ceremony.** The sixth answer, right or wrong,
/// finds nothing: the person signs in again.
#[tokio::test(flavor = "multi_thread")]
async fn five_wrong_codes_end_the_ceremony() {
    let t = totp_harness(SecondFactorConfig::default());
    let app = t.h.app();
    let secret = enrol_totp(&app, &token_by_password(&app).await, NOW).await;
    t.h.set_now(NOW + 30);
    let (_, half) = password_first_step(&app).await;
    for _ in 0..5 {
        let (status, _) = answer_totp(&app, &half, "000000").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let right = totp::totp_at(&secret, NOW + 30, 6);
    let (status, body) = answer_totp(&app, &half, &right).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the ceremony is over: {body}"
    );
    let kinds = reasons(&drained(&t.h));
    assert!(
        kinds
            .iter()
            .any(|k| k.contains("reason=second-step-attempts-exhausted")),
        "{kinds:?}"
    );
    // A fresh sign-in starts again, with the code still good.
    let (_, half) = password_first_step(&app).await;
    let (status, _) = answer_totp(&app, &half, &right).await;
    assert_eq!(status, StatusCode::OK);
}

/// **Twenty consecutive failures disable the factor, and the stream says
/// so.** Across ceremonies, not within one. A locked factor still counts as
/// held, so a password alone does not sign in, and it refuses the right code.
/// The number is the deployment's: a deployment that says three locks at three.
#[tokio::test(flavor = "multi_thread")]
async fn twenty_consecutive_failures_disable_the_factor_and_the_stream_says_so() {
    for (lock_after, per_ceremony, ceremonies) in [(20u32, 5u32, 4u32), (3, 5, 1)] {
        let config = SecondFactorConfig {
            lock_after,
            attempts_per_ceremony: per_ceremony,
            ..SecondFactorConfig::default()
        };
        let t = totp_harness(config);
        let app = t.h.app();
        let secret = enrol_totp(&app, &token_by_password(&app).await, NOW).await;
        t.h.set_now(NOW + 30);
        let mut wrong = 0;
        'all: for _ in 0..ceremonies {
            let (_, half) = password_first_step(&app).await;
            for _ in 0..per_ceremony {
                let (status, _) = answer_totp(&app, &half, "000000").await;
                assert_eq!(status, StatusCode::UNAUTHORIZED);
                wrong += 1;
                if wrong == lock_after {
                    break 'all;
                }
            }
        }
        assert_eq!(wrong, lock_after);
        let kinds = reasons(&drained(&t.h));
        assert_eq!(
            kinds
                .iter()
                .filter(|k| k.starts_with("second-factor disabled"))
                .count(),
            1,
            "the lock is recorded once: {kinds:?}"
        );
        assert!(kinds.iter().any(|k| k.contains("kind=totp")));

        // Locked: the right code is refused, and a password alone is not a way
        // in. The person is still owed the step.
        let (_, half) = password_first_step(&app).await;
        assert!(half.get("token").is_none(), "a locked factor still counts");
        let right = totp::totp_at(&secret, NOW + 30, 6);
        let (status, _) = answer_totp(&app, &half, &right).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "locked at {lock_after}");
    }
}

/// **An unconfirmed secret enrols nothing.** A begin that is never confirmed,
/// a confirm with a wrong code, and a confirm after the secret expired each
/// leave the person with no factor and a password that signs in alone.
#[tokio::test(flavor = "multi_thread")]
async fn an_unconfirmed_secret_enrols_nothing() {
    let t = totp_harness(SecondFactorConfig::default());
    let app = t.h.app();
    let token = token_by_password(&app).await;
    let (_, begun) = begin_enrolment(
        &app,
        "/auth/factors/totp/begin",
        &token,
        json!({}),
        None,
        NOW,
    )
    .await;
    let secret = unbase32(begun["secret"].as_str().expect("secret"));

    // A wrong code confirms nothing.
    let (status, _) = send(
        &app,
        "POST",
        "/auth/factors/totp/confirm",
        Some(json!({"factor": begun["factor"], "code": "000000"})),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(t.store.all().is_empty());

    // The secret expires, and then even the right code confirms nothing.
    t.h.set_now(NOW + 301);
    let (status, _) = send(
        &app,
        "POST",
        "/auth/factors/totp/confirm",
        Some(json!({"factor": begun["factor"],
                    "code": totp::totp_at(&secret, NOW + 301, 6)})),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "expired");
    assert!(t.store.all().is_empty(), "nothing enrolled");

    // The person still signs in by password alone.
    let (status, signed) = password_first_step(&app).await;
    assert_eq!(status, StatusCode::OK, "{signed}");
    assert!(signed["token"].as_str().is_some(), "{signed}");
}

// ══════════════════════════ T4 · backup codes as the second step ══════════

use ordo_host::recovery::{MemoryRecoveryStore, Recovery, RecoveryPolicy};
use ordo_host::recovery_codes::{MemoryCodeStore, RecoveryCodes};
use ordo_host::second_factors::BackupCodeMethod;
use ordo_host::tickets::{MemoryTicketStore, Tickets};

struct Full {
    h: Harness,
    recovery: Arc<Recovery>,
    store: Arc<MemorySecondFactorStore>,
    code_store: Arc<MemoryCodeStore>,
    codes: Arc<RecoveryCodes>,
}

/// A deployment with code-generating factors and backup codes, and recovery
/// wired the way the binary wires it, so a code can be spent either way.
fn full_harness(config: SecondFactorConfig) -> Full {
    let clock = Arc::new(AtomicU64::new(NOW));
    let store = Arc::new(MemorySecondFactorStore::new());
    let read = clock.clone();
    let code_store = Arc::new(MemoryCodeStore::new());
    let codes = Arc::new(
        RecoveryCodes::new(code_store.clone())
            .with_clock(Box::new(move || read.load(Ordering::SeqCst))),
    );
    let totp = Arc::new(TotpMethod::new(
        store.clone(),
        Sealer::new(Arc::new(ordo_crypto::TestProvider::new())),
        config.clone(),
    ));
    let backup = Arc::new(BackupCodeMethod::new(codes.clone(), store.clone(), &config));
    let sf = SecondFactors::new(config)
        .with_totp(totp)
        .with_backup(backup);
    let h = harness_with(
        clock.clone(),
        Arc::new(ordo_identity::MemoryCeremonyStore::new()),
        sf,
    );
    let read = clock.clone();
    let tickets = Arc::new(
        Tickets::new(Arc::new(MemoryTicketStore::new()))
            .with_clock(Box::new(move || read.load(Ordering::SeqCst))),
    );
    let read = clock.clone();
    let recovery = Recovery::new(
        Arc::new(MemoryRecoveryStore::new()),
        tickets.clone(),
        RecoveryPolicy::default(),
    )
    .with_clock(Box::new(move || read.load(Ordering::SeqCst)))
    .with_recorder(h.auth.recorder.clone().expect("recorder"))
    .with_codes(codes.clone())
    .with_tenants(Box::new(|_| Vec::new()))
    // A recovery removes every second factor once it completes.
    .with_settlement(Arc::new(ordo_host::second_factors::RemoveSecondFactors {
        store: store.clone(),
        passkey: h.auth.passkey.clone(),
        recorder: h.auth.recorder.clone(),
    }));
    let recovery = Arc::new(recovery);
    h.auth.wire_enrolment(ordo_host::auth::Enrolment {
        tickets: Some(tickets),
        ..Default::default()
    });
    h.auth.wire_recovery(recovery.clone());
    Full {
        recovery,
        h,
        store,
        code_store,
        codes,
    }
}

/// A fresh proof for a person who is signed in as `token`: the first factor
/// again (`reauth: true`), the second step when they hold a factor, and the
/// handle that comes out. `totp_secret` answers the second step.
async fn reauth_handle(
    app: &axum::Router,
    token: &str,
    totp_secret: Option<&[u8]>,
    at: u64,
) -> String {
    let (_, begun) = post(
        app,
        "/auth/begin",
        json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let (status, first) = send(
        app,
        "POST",
        "/auth/complete",
        Some(
            json!({"method": "password", "kind": "password", "reauth": true,
                    "challenge": begun["challenge"], "identifier": "ana@example.test",
                    "password": "correct-horse"}),
        ),
        Some(token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert!(
        first.get("token").is_none(),
        "a reauth never mints: {first}"
    );
    if let Some(handle) = first["reauth"].as_str() {
        return handle.to_string();
    }
    let secret = totp_secret.expect("a second step is owed and there is no secret");
    let (status, done) = answer_totp(app, &first, &totp::totp_at(secret, at, 6)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert!(done.get("token").is_none(), "a reauth never mints: {done}");
    done["reauth"].as_str().expect("a handle").to_string()
}

async fn issue_with(app: &axum::Router, token: &str, handle: Option<&str>) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method("POST")
        .uri("/auth/recovery-codes")
        .header("authorization", format!("Bearer {token}"));
    if let Some(h) = handle {
        b = b.header("x-reauth", h);
    }
    let res = app
        .clone()
        .oneshot(b.body(Body::empty()).expect("request"))
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

/// A person with a password, a code-generating factor and a set of backup
/// codes, and the secret and the codes.
async fn person_with_codes(f: &Full, app: &axum::Router) -> (String, Vec<u8>, Vec<String>) {
    let token = token_by_password(app).await;
    let secret = enrol_totp(app, &token, NOW).await;
    f.h.set_now(NOW + 30);
    let handle = reauth_handle(app, &token, Some(&secret), NOW + 30).await;
    // The handle is the only thing that lets a password session hold codes.
    let (status, body) = issue_with(app, &token, Some(&handle)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let codes = body["codes"]
        .as_array()
        .expect("codes")
        .iter()
        .map(|c| c.as_str().expect("code").to_string())
        .collect();
    (token, secret, codes)
}

async fn answer_code(app: &axum::Router, half: &Value, code: &str) -> (StatusCode, Value) {
    post(
        app,
        "/auth/second-step",
        json!({"handle": half["second_step"], "binder": half["binder"],
               "kind": "backup_code", "code": code}),
    )
    .await
}

/// **A backup code completes the second step, once.** It is offered beside the
/// person's factor, it signs in with no presence, it is spent, the answer says
/// how many are left and says so loudly at two, and the stream records the use.
#[tokio::test(flavor = "multi_thread")]
async fn a_backup_code_completes_the_second_step_once() {
    let f = full_harness(SecondFactorConfig::default());
    let app = f.h.app();
    let (_, _secret, codes) = person_with_codes(&f, &app).await;
    assert_eq!(codes.len(), 10);

    let (_, half) = password_first_step(&app).await;
    assert_eq!(half["methods"], json!(["totp", "backup-code"]));
    let (status, done) = answer_code(&app, &half, &codes[0]).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["second_step"], "backup-code");
    assert_eq!(done["recovery_codes_remaining"], 9);
    assert_eq!(done["recovery_codes_low"], false);
    let claims = claims_of(done["token"].as_str().expect("token"), NOW + 30);
    assert_eq!(
        claims.session.and_then(|s| s.second_step),
        Some(SecondStepKind::BackupCode)
    );
    assert!(
        claims.user_presence_at.is_none(),
        "a code carries no presence"
    );

    // The same code, on a fresh sign-in, is spent.
    let (_, half) = password_first_step(&app).await;
    let (status, _) = answer_code(&app, &half, &codes[0]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "one use");

    // Loosely typed, it is the same code; the count falls to two and the
    // answer says so.
    let mut last = Value::Null;
    for code in &codes[1..8] {
        let (_, half) = password_first_step(&app).await;
        let (status, done) = answer_code(&app, &half, &code.to_lowercase().replace('-', " ")).await;
        assert_eq!(status, StatusCode::OK, "{done}");
        last = done;
    }
    assert_eq!(last["recovery_codes_remaining"], 2);
    assert_eq!(last["recovery_codes_low"], true, "loud at two or fewer");

    // Every use is in the stream as a second-step use, and none as recovery.
    let kinds = reasons(&drained(&f.h));
    let used: Vec<&String> = kinds
        .iter()
        .filter(|k| k.starts_with("recovery code-used"))
        .collect();
    assert!(!used.is_empty(), "{kinds:?}");
    assert!(used.iter().all(|k| k.contains("used_as=second-step")));
}

/// **A backup code at `/auth/complete`, with no first factor, is the one
/// answer.** It is not a method. Neither does a code given to the second step
/// with a handle nobody minted spend anything.
#[tokio::test(flavor = "multi_thread")]
async fn a_backup_code_at_auth_complete_without_a_first_factor_is_the_one_answer() {
    let f = full_harness(SecondFactorConfig::default());
    let app = f.h.app();
    let (token, _, codes) = person_with_codes(&f, &app).await;
    let ana = claims_of(&token, NOW + 30).principal.0;

    let (status, alone) = post(
        &app,
        "/auth/complete",
        json!({"method": "backup_code", "kind": "backup_code", "code": codes[0]}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{alone}");
    assert!(alone.get("token").is_none());
    let (_, begun) = post(
        &app,
        "/auth/begin",
        json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let (_, wrong_password) = post(
        &app,
        "/auth/complete",
        json!({"method": "password", "kind": "password",
               "challenge": begun["challenge"], "identifier": "ana@example.test",
               "password": "wrong"}),
    )
    .await;
    assert_eq!(alone, wrong_password, "the same answer as a wrong password");

    // A handle nobody minted, with a code that is good: nothing is spent.
    let (status, _) = post(
        &app,
        "/auth/second-step",
        json!({"handle": "ss_nothing", "binder": "0".repeat(64),
               "kind": "backup_code", "code": codes[0]}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(f.codes.remaining(&ana).expect("count"), 10, "not spent");
}

/// **A code spent as a second step cannot open a recovery.** The table says how
/// it was spent, and recovery refuses it.
#[tokio::test(flavor = "multi_thread")]
async fn a_code_spent_as_a_second_step_cannot_open_a_recovery() {
    let f = full_harness(SecondFactorConfig::default());
    let app = f.h.app();
    let (_, _, codes) = person_with_codes(&f, &app).await;
    let (_, half) = password_first_step(&app).await;
    let (status, done) = answer_code(&app, &half, &codes[0]).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let hash = ordo_host::recovery_codes::hash_of(
        &ordo_host::recovery_codes::normalize(&codes[0]).expect("a code"),
    );
    assert_eq!(f.code_store.used_as(&hash).as_deref(), Some("second-step"));

    let (status, body) = post(&app, "/auth/recover/code", json!({"code": codes[0]})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    // An unused one still opens a recovery and returns its ticket: the same set,
    // two uses.
    let (status, body) = post(&app, "/auth/recover/code", json!({"code": codes[1]})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["ticket"].as_str().is_some());
}

/// **A person with a password and no passkey can issue codes** with a fresh
/// first factor, and a second step behind it when they hold a factor. A
/// bearer alone cannot, a handle is good once, and a reauth is never a session.
#[tokio::test(flavor = "multi_thread")]
async fn a_password_only_person_can_issue_codes_with_a_fresh_first_factor_and_a_second_step() {
    let f = full_harness(SecondFactorConfig::default());
    let app = f.h.app();
    let token = token_by_password(&app).await;

    // A bearer alone: refused, and told what to do.
    let (status, body) = issue_with(&app, &token, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "presence_required");

    // A person with no factor: the first factor alone gives the handle.
    let handle = reauth_handle(&app, &token, None, NOW).await;
    let (status, body) = issue_with(&app, &token, Some(&handle)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 10);
    // One act's worth: the same handle is spent.
    let (status, _) = issue_with(&app, &token, Some(&handle)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a handle is one-shot");

    // A reauth needs the bearer of the same session, and never mints.
    let (_, begun) = post(
        &app,
        "/auth/begin",
        json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let (status, _) = post(
        &app,
        "/auth/complete",
        json!({"method": "password", "kind": "password", "reauth": true,
               "challenge": begun["challenge"], "identifier": "ana@example.test",
               "password": "correct-horse"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no bearer, no reauth");

    // Once a factor is enrolled the first factor alone is not enough: the
    // ceremony asks for the second step and only then answers a handle.
    let secret = enrol_totp(&app, &token, NOW).await;
    f.h.set_now(NOW + 30);
    let (_, begun) = post(
        &app,
        "/auth/begin",
        json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let (status, first) = send(
        &app,
        "POST",
        "/auth/complete",
        Some(
            json!({"method": "password", "kind": "password", "reauth": true,
                    "challenge": begun["challenge"], "identifier": "ana@example.test",
                    "password": "correct-horse"}),
        ),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert!(first.get("reauth").is_none(), "not yet: {first}");
    assert_eq!(first["methods"], json!(["totp", "backup-code"]));
    let (status, done) = answer_totp(&app, &first, &totp::totp_at(&secret, NOW + 30, 6)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let handle = done["reauth"].as_str().expect("a handle after the step");
    let (status, body) = issue_with(&app, &token, Some(handle)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let _ = &f.store;
}

/// A person's wrong backup codes are counted against them, and at the limit
/// their code redemption is locked until a fresh set is issued. The
/// code-generating factor is untouched by it.
#[tokio::test(flavor = "multi_thread")]
async fn wrong_backup_codes_lock_after_the_limit_and_a_fresh_set_clears_it() {
    let f = full_harness(SecondFactorConfig {
        lock_after: 3,
        ..SecondFactorConfig::default()
    });
    let app = f.h.app();
    let (token, secret, codes) = person_with_codes(&f, &app).await;
    let (_, half) = password_first_step(&app).await;
    for _ in 0..3 {
        let (status, _) = answer_code(&app, &half, &"0".repeat(24)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let kinds = reasons(&drained(&f.h));
    assert!(
        kinds.iter().any(|k| k.contains("second-factor disabled")),
        "{kinds:?}"
    );
    // Locked: a right code is refused.
    let (_, half) = password_first_step(&app).await;
    let (status, _) = answer_code(&app, &half, &codes[0]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "locked");
    // The code-generating factor still works, and issuing a fresh set unlocks.
    f.h.set_now(NOW + 60);
    let (_, half) = password_first_step(&app).await;
    let (status, _) = answer_totp(&app, &half, &totp::totp_at(&secret, NOW + 60, 6)).await;
    assert_eq!(status, StatusCode::OK);
    f.h.set_now(NOW + 90);
    let handle = reauth_handle(&app, &token, Some(&secret), NOW + 90).await;
    let (status, body) = issue_with(&app, &token, Some(&handle)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let fresh = body["codes"][0].as_str().expect("a code").to_string();
    let (_, half) = password_first_step(&app).await;
    let (status, done) = answer_code(&app, &half, &fresh).await;
    assert_eq!(status, StatusCode::OK, "a fresh set unlocks: {done}");
}

// ══════════════════════════ T3 · security keys as a second factor ═════════

use ordo_api::{AccessChecker, AccessDecision};
use ordo_host::metrics::Metrics;
use ordo_host::second_factors::SecurityKeyMethod;
use ordo_host::IdentityAccess;
use ordo_identity::provider::CompleteRequest;
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{AccessEngine, AccessProfile, Role, SpaceId};

const SPACE: &str = "spc_clinic";
/// Stands for `std.space.export`: an owner-only op that asks for presence.
const GATED: &str = "space.export";
const PLAIN: &str = "records.list";

struct Keys {
    h: Harness,
    store: Arc<MemorySecondFactorStore>,
    passkey: Arc<PasskeyProvider>,
}

/// A deployment whose second step is security keys, over the same passkey
/// provider the sign-in routes use.
fn key_harness() -> Keys {
    let clock = Arc::new(AtomicU64::new(NOW));
    let ceremonies: Arc<dyn ordo_identity::CeremonyStore> =
        Arc::new(ordo_identity::MemoryCeremonyStore::new());
    let passkey = passkey_over(ceremonies.clone());
    let store = Arc::new(MemorySecondFactorStore::new());
    let config = SecondFactorConfig::default();
    let key = Arc::new(SecurityKeyMethod::new(
        store.clone(),
        passkey.clone(),
        config.clone(),
    ));
    let sf = SecondFactors::new(config).with_key(key);
    let h = harness_passkey(clock, ceremonies, passkey.clone(), sf);
    Keys { h, store, passkey }
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

fn registration_body(req: &CompleteRequest) -> Value {
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
        "display": "Ana", "label": "my key",
    })
}

/// A key with no PIN: it proves presence of a finger and verifies nobody.
fn plain_key() -> TestAuthenticator {
    TestAuthenticator {
        user_verified: false,
        ..TestAuthenticator::es256()
    }
}

/// Register `device` as a second factor with a session's bearer.
async fn enrol_key(app: &axum::Router, token: &str, device: &TestAuthenticator) -> Value {
    let (status, options) = begin_enrolment(
        app,
        "/auth/factors/key/begin",
        token,
        json!({"label": "my key"}),
        None,
        NOW,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(options["resident_key"], "discouraged");
    assert_eq!(options["user_verification"], "preferred");
    assert_eq!(options["attestation"], "none");
    let challenge = options["challenge"].as_str().expect("challenge");
    let (status, done) = send(
        app,
        "POST",
        "/auth/factors/key/complete",
        Some(registration_body(&device.register(challenge))),
        Some(token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done
}

/// Answer a second step with `device`: the challenge from `begin`, then the
/// assertion.
async fn answer_key(
    app: &axum::Router,
    half: &Value,
    device: &TestAuthenticator,
) -> (StatusCode, Value) {
    let (status, options) = post(
        app,
        "/auth/second-step/begin",
        json!({"handle": half["second_step"], "binder": half["binder"],
               "kind": "security-key"}),
    )
    .await;
    if status != StatusCode::OK {
        return (status, options);
    }
    let challenge = options["challenge"].as_str().expect("challenge");
    let mut body = assertion_body(&device.assert(challenge));
    body["handle"] = half["second_step"].clone();
    body["binder"] = half["binder"].clone();
    body["kind"] = json!("passkey_assertion");
    post(app, "/auth/second-step", body).await
}

/// **A second-factor key completes the second step.** Registered without
/// attestation and without a PIN, offered beside nothing else, asked for by
/// name with its own challenge, and it signs in a person whose password
/// alone would not.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_factor_key_completes_the_second_step() {
    let k = key_harness();
    let app = k.h.app();
    let device = plain_key();
    let token = token_by_password(&app).await;
    let enrolled = enrol_key(&app, &token, &device).await;
    assert_eq!(enrolled["kind"], "security-key");
    assert_eq!(enrolled["user_verified"], false, "the record says so");
    assert_eq!(k.store.all().len(), 1);

    let (_, half) = password_first_step(&app).await;
    assert!(half.get("token").is_none(), "a key is enrolled: {half}");
    assert_eq!(half["methods"], json!(["security-key"]));
    // The challenge is narrowed to the person's own key.
    let (_, options) = post(
        &app,
        "/auth/second-step/begin",
        json!({"handle": half["second_step"], "binder": half["binder"], "kind": "security-key"}),
    )
    .await;
    assert_eq!(
        options["allow_credentials"],
        json!([b64url_encode(&device.credential_id)])
    );
    assert_eq!(options["user_verification"], "preferred");

    let (status, done) = answer_key(&app, &half, &device).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["second_step"], "security-key");
    let claims = claims_of(done["token"].as_str().expect("token"), NOW);
    assert_eq!(
        claims.session.and_then(|s| s.second_step),
        Some(SecondStepKind::SecurityKey)
    );
    assert!(claims.user_presence_at.is_none());

    // A begin without the binder is the one answer, and mints no challenge.
    let (_, half) = password_first_step(&app).await;
    let (status, _) = post(
        &app,
        "/auth/second-step/begin",
        json!({"handle": half["second_step"], "binder": "0".repeat(64), "kind": "security-key"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The record names the kind, the user verification, and no model.
    let kinds = reasons(&drained(&k.h));
    let enrolled: Vec<&String> = kinds
        .iter()
        .filter(|k| k.starts_with("second-factor enrolled"))
        .collect();
    assert_eq!(enrolled.len(), 1, "{kinds:?}");
    assert!(enrolled[0].contains("kind=security-key"));
    assert!(enrolled[0].contains("user_verified=no"));
}

/// **A second-factor key cannot sign in alone.** Its assertion at the sign-in
/// route is refused as any bad credential is, it cannot step up, and a
/// first-factor passkey cannot answer a second step either. No person is made
/// by trying.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_factor_key_cannot_sign_in_alone() {
    let k = key_harness();
    let app = k.h.app();
    let device = TestAuthenticator::es256();
    let token = token_by_password(&app).await;
    enrol_key(&app, &token, &device).await;

    // The sign-in route: the same answer as an assertion nobody enrolled.
    let (_, begun) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let (status, alone) = post(
        &app,
        "/auth/complete",
        assertion_body(&device.assert(begun["challenge"].as_str().expect("challenge"))),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{alone}");
    assert!(alone.get("token").is_none());
    let stranger = TestAuthenticator::ed25519();
    let (_, begun) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let (_, unknown) = post(
        &app,
        "/auth/complete",
        assertion_body(&stranger.assert(begun["challenge"].as_str().expect("challenge"))),
    )
    .await;
    assert_eq!(alone, unknown, "the same answer as a key nobody enrolled");

    // The step-up route refuses it too.
    let (_, options) = send(&app, "POST", "/auth/step-up/begin", None, Some(&token)).await;
    let (status, _) = send(
        &app,
        "POST",
        "/auth/step-up",
        Some(assertion_body(
            &device.assert(options["challenge"].as_str().expect("challenge")),
        )),
        Some(&token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "no presence from a second factor"
    );

    // A first-factor passkey does not answer a second step.
    let passkey_device = TestAuthenticator {
        credential_id: b"credential-first".to_vec(),
        ..TestAuthenticator::ed25519()
    };
    let (_, options) = post(&app, "/auth/register", json!({"method": "passkey"})).await;
    let (status, _) = post(
        &app,
        "/auth/complete",
        registration_body(
            &passkey_device.register(options["challenge"].as_str().expect("challenge")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "a first-factor passkey registers");
    let (_, half) = password_first_step(&app).await;
    let (status, done) = answer_key(&app, &half, &passkey_device).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{done}");

    // Nothing was made by any of it: the key is on `$secondfactor` only.
    assert_eq!(k.store.all().len(), 1);
    assert!(
        k.passkey.credential(&device.credential_id).is_some(),
        "the credential is stored, with its role, and nothing else"
    );
}

/// A door over an engine where `principal` owns `SPACE`, `space.export` asks
/// for presence and `records.list` does not.
fn door_for(principal: &ordo_identity::PrincipalId, at: u64) -> IdentityAccess {
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
}

/// **A password plus a key cannot pass step-up.** The design rule: a second
/// factor never carries presence. The control is the same kind of key
/// registered as a passkey through `/auth/register`, which can.
#[tokio::test(flavor = "multi_thread")]
async fn a_password_plus_key_session_cannot_pass_step_up() {
    let k = key_harness();
    let app = k.h.app();
    let device = TestAuthenticator::es256();
    let token = token_by_password(&app).await;
    enrol_key(&app, &token, &device).await;
    let (_, half) = password_first_step(&app).await;
    let (status, done) = answer_key(&app, &half, &device).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let session = done["token"].as_str().expect("token").to_string();
    let ana = claims_of(&session, NOW).principal;

    let door = door_for(&ana, NOW + 1);
    assert!(
        matches!(
            door.check(&session, SPACE, PLAIN),
            AccessDecision::Allow { .. }
        ),
        "ordinary calls work"
    );
    assert_eq!(
        door.check(&session, SPACE, GATED),
        AccessDecision::Deny,
        "a password and a key are two factors and neither is presence"
    );

    // The control: the same sort of key, registered as a passkey through the
    // sign-up route, signs in with presence and passes.
    let control = TestAuthenticator {
        credential_id: b"credential-control".to_vec(),
        ..TestAuthenticator::ed25519()
    };
    let (_, options) = post(&app, "/auth/register", json!({"method": "passkey"})).await;
    let (status, signed) = post(
        &app,
        "/auth/complete",
        registration_body(&control.register(options["challenge"].as_str().expect("challenge"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{signed}");
    let passkey_session = signed["token"].as_str().expect("token");
    let bea = claims_of(passkey_session, NOW).principal;
    let door = door_for(&bea, NOW + 1);
    assert!(
        matches!(
            door.check(passkey_session, SPACE, GATED),
            AccessDecision::Allow { .. }
        ),
        "the passkey carries presence"
    );
}

// ══════════════════════════ T5 · who requires it ══════════════════════════

use ordo_host::second_factor_store::SecondFactorStore;
use ordo_host::second_factors::SecondStepLevels;
use ordo_identity::second_step::{DeploymentMode, PersonMode, TenantMode};
use ordo_secevent::{EventKind, RecordedDecision};

const OTHER_SPACE: &str = "spc_other";

/// A deployment whose second step is code-generating factors, at the given
/// settings, with the levels and the person's store exposed.
struct Layers {
    h: Harness,
    store: Arc<MemorySecondFactorStore>,
    levels: Arc<SecondStepLevels>,
}

fn layers(config: SecondFactorConfig) -> Layers {
    let store = Arc::new(MemorySecondFactorStore::new());
    let totp = Arc::new(TotpMethod::new(
        store.clone(),
        Sealer::new(Arc::new(ordo_crypto::TestProvider::new())),
        config.clone(),
    ));
    let sf = SecondFactors::new(config)
        .with_store(store.clone())
        .with_totp(totp);
    let levels = sf.levels().clone();
    let h = harness_with(
        Arc::new(AtomicU64::new(NOW)),
        Arc::new(ordo_identity::MemoryCeremonyStore::new()),
        sf,
    );
    Layers { h, store, levels }
}

/// A door over an engine where `principal` owns two spaces and both ops are
/// open to owners, with the second-step levels wired in.
fn door_over(
    principal: &ordo_identity::PrincipalId,
    levels: &Arc<SecondStepLevels>,
    at: u64,
) -> IdentityAccess {
    let mut engine = AccessEngine::new();
    for space in [SPACE, OTHER_SPACE] {
        let space = SpaceId(space.into());
        engine.set_profile(
            &space,
            AccessProfile::new()
                .allow(Role::Owner, PLAIN)
                .allow(Role::Owner, GATED),
        );
        engine.set_membership(&space, principal, Role::Owner);
    }
    let door = IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(move || at),
        Arc::new(Metrics::new()),
    );
    door.wire_second_step(levels.clone());
    door
}

fn allowed(door: &IdentityAccess, token: &str, space: &str) -> bool {
    matches!(
        door.check(token, space, PLAIN),
        AccessDecision::Allow { .. }
    )
}

/// Sign in with the password, naming `space`, and answer the second step with
/// the code for `at`. The token, or the answer that was not one.
async fn password_naming(
    app: &axum::Router,
    space: &str,
    secret: &[u8],
    at: u64,
) -> Result<String, Value> {
    let (_, begun) = post(
        app,
        "/auth/begin",
        json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let (_, half) = post(
        app,
        "/auth/complete",
        json!({"method": "password", "kind": "password", "space": space,
               "challenge": begun["challenge"], "identifier": "ana@example.test",
               "password": "correct-horse", "display": "Ana"}),
    )
    .await;
    if half.get("second_step").is_none() {
        return Err(half);
    }
    let (status, done) = answer_totp(app, &half, &totp::totp_at(secret, at, 6)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    Ok(done["token"].as_str().expect("token").to_string())
}

/// **A tenant that requires a second factor refuses a session that skipped it,
/// at the door.** The session was made before the tenant asked, or without
/// naming it. The tenant's neighbour, which asks nothing, is not affected, and
/// signing in again naming the tenant takes the step and passes.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_that_requires_a_second_factor_refuses_a_session_that_skipped_it_at_the_door() {
    let l = layers(SecondFactorConfig::default());
    let app = l.h.app();
    let token = token_by_password(&app).await;
    let secret = enrol_totp(&app, &token, NOW).await;
    let ana = claims_of(&token, NOW).principal;
    let door = door_over(&ana, &l.levels, NOW + 1);

    // Before the tenant asks, the session passes.
    assert!(allowed(&door, &token, SPACE));

    // The tenant's owner requires it.
    l.levels.set(SPACE, TenantMode::Required);
    let _ = door.take_events();
    assert!(
        !allowed(&door, &token, SPACE),
        "the session skipped the step"
    );
    assert_eq!(
        door.take_events().last().expect("an event").kind,
        EventKind::denied(RecordedDecision::SecondStepRequired),
        "refused for that, and the stream says so"
    );
    assert!(
        allowed(&door, &token, OTHER_SPACE),
        "a tenant that asks nothing is not affected"
    );

    // Signing in again, naming the tenant, takes the step, and passes.
    l.h.set_now(NOW + 30);
    let door = door_over(&ana, &l.levels, NOW + 31);
    let session = password_naming(&app, SPACE, &secret, NOW + 30)
        .await
        .expect("the tenant that requires it asks for the step");
    assert!(allowed(&door, &session, SPACE));
    // Naming a tenant that asks nothing, a password with a factor still owes it
    // (the person turned it on by enrolling), and a person with none owes nothing.
    assert!(password_naming(&app, OTHER_SPACE, &secret, NOW + 60)
        .await
        .is_ok());
}

/// **A passkey session is exempt unless the tenant or the person says
/// otherwise.** Default off, because a passkey already needs the device and its
/// unlock. A tenant that requires it for everyone, or a person who turned it on
/// for their passkeys, gets the step after a passkey too.
#[tokio::test(flavor = "multi_thread")]
async fn a_passkey_session_is_exempt_unless_the_tenant_or_the_person_says_otherwise() {
    let l = layers(SecondFactorConfig::default());
    let app = l.h.app();
    let device = TestAuthenticator::es256();
    // Sign up with a passkey and enrol a factor with its session.
    let (_, options) = post(&app, "/auth/register", json!({"method": "passkey"})).await;
    let (status, signed) = post(
        &app,
        "/auth/complete",
        registration_body(&device.register(options["challenge"].as_str().expect("challenge"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{signed}");
    let first = signed["token"].as_str().expect("token").to_string();
    let secret = enrol_totp(&app, &first, NOW).await;
    let ana = claims_of(&first, NOW).principal;
    l.h.set_now(NOW + 30);

    let passkey_in = |space: Option<&'static str>| {
        let app = app.clone();
        let device = &device;
        async move {
            let (_, begun) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
            let mut body =
                assertion_body(&device.assert(begun["challenge"].as_str().expect("challenge")));
            if let Some(space) = space {
                body["space"] = json!(space);
            }
            post(&app, "/auth/complete", body).await.1
        }
    };
    let door = door_over(&ana, &l.levels, NOW + 31);

    // The default: a passkey signs in with no second step, though a factor is
    // enrolled, and passes a tenant that requires it only of everything but
    // passkeys.
    let signed = passkey_in(None).await;
    let plain = signed["token"]
        .as_str()
        .expect("no step: {signed}")
        .to_string();
    l.levels.set(SPACE, TenantMode::RequiredForNonPasskey);
    assert!(allowed(&door, &plain, SPACE), "exempt at the door");
    let signed = passkey_in(Some(SPACE)).await;
    assert!(
        signed["token"].as_str().is_some(),
        "exempt at sign-in: {signed}"
    );

    // The tenant requires it of everyone: the same passkey session is refused
    // at the door, and a passkey sign-in naming the tenant is asked for the step.
    l.levels.set(SPACE, TenantMode::Required);
    assert!(!allowed(&door, &plain, SPACE), "the tenant said otherwise");
    let half = passkey_in(Some(SPACE)).await;
    assert!(half.get("token").is_none(), "{half}");
    assert_eq!(half["methods"], json!(["totp"]));
    let (status, done) = answer_totp(&app, &half, &totp::totp_at(&secret, NOW + 30, 6)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let stepped = done["token"].as_str().expect("token").to_string();
    assert!(allowed(&door, &stepped, SPACE));
    assert!(
        claims_of(&stepped, NOW + 30).user_presence_at.is_some(),
        "the passkey's presence is carried, and the step adds none"
    );

    // The person says otherwise for themselves: a passkey sign-in owes the step
    // with no tenant asking.
    l.levels.set(SPACE, TenantMode::Inherit);
    let signed = passkey_in(None).await;
    assert!(signed["token"].as_str().is_some(), "{signed}");
    l.store
        .set_person_mode(&ana.0, PersonMode::AlsoForPasskeys, NOW)
        .expect("stored");
    let half = passkey_in(None).await;
    assert!(
        half.get("token").is_none(),
        "the person turned it on: {half}"
    );
    assert_eq!(half["methods"], json!(["totp"]));
}

/// **A tenant cannot require what the deployment turned off**, and cannot
/// loosen what it requires. Refused when the owner sets it, by name, and never
/// stored; under a deployment that allows it, the choice is stored and is the
/// level the next request meets.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_cannot_require_what_the_deployment_turned_off() {
    use ordo_api::{BoundParams, ErrorCode};
    use ordo_host::admission::{Admission, Ceiling, PolicyChange, PolicyStore, SET_ADMISSION};
    use ordo_host::member_ops::MemberOps;

    async fn admission_for(db: &str, mode: DeploymentMode) -> (Admission, Arc<SecondStepLevels>) {
        let f = ordo_conformance::Fixture::load().expect("fixture");
        let pg = ordo_conformance::Pg::named(db);
        f.reset(&pg).expect("seeded");
        let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
            db: db.to_string(),
            size: 2,
            ..ordo_exec::PoolConfig::from_env()
        })
        .await
        .expect("pool");
        ordo_host::admission::ensure_table(&pool)
            .await
            .expect("policy table");
        let _ = pool.rows("DELETE FROM \"$admissionpolicy\"").await;
        let members = Arc::new(MemberOps {
            control: pool.clone(),
            pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
            directory: None,
            stream: Some(Arc::default()),
            refresh: None,
            now: Arc::new(|| NOW),
            managed: None,
        });
        let levels = Arc::new(SecondStepLevels::new(mode));
        (
            Admission {
                members,
                store: PolicyStore::new(pool).with_levels(levels.clone()),
                tickets: None,
                ceiling: Ceiling::everything(),
                home: None,
            },
            levels,
        )
    }
    let space = ordo_conformance::fixture_space().0;
    let change = |mode: TenantMode| PolicyChange {
        second_factor: Some(mode),
        ..Default::default()
    };

    // Off: nothing can be required, and nothing is stored.
    let (off, levels) = admission_for("ordo_secondfactor_off", DeploymentMode::Off).await;
    for mode in [TenantMode::Required, TenantMode::RequiredForNonPasskey] {
        let err = off
            .set_policy(&space, &change(mode), "mem_o", NOW)
            .await
            .expect_err("the deployment turned it off");
        assert_eq!(err.code, ErrorCode::TenantRefused);
        assert!(
            err.message.contains("turned the second step off"),
            "{}",
            err.message
        );
    }
    assert_eq!(levels.tenant(&space), TenantMode::Inherit);
    assert_eq!(
        off.store.get(&space).await.expect("read").second_factor,
        TenantMode::Inherit,
        "nothing was stored"
    );
    off.set_policy(&space, &change(TenantMode::Inherit), "mem_o", NOW)
        .await
        .expect("inheriting is always allowed");

    // Required at the deployment: a tenant cannot loosen it to passkey-exempt.
    let (req, _) = admission_for("ordo_secondfactor_req", DeploymentMode::Required).await;
    assert!(req
        .set_policy(
            &space,
            &change(TenantMode::RequiredForNonPasskey),
            "mem_o",
            NOW
        )
        .await
        .is_err());

    // Enabled: the tenant's choice is stored, is the live level, and is what the
    // op answers. A word that is not a level is refused.
    let (on, levels) = admission_for("ordo_secondfactor_on", DeploymentMode::Enabled).await;
    let bound = |v: Value| BoundParams(v.as_object().expect("object").clone());
    let done = on
        .execute(
            SET_ADMISSION,
            &space,
            &bound(json!({"secondFactor": "required"})),
            "mem_o",
        )
        .await
        .expect("a tenant may require it");
    assert_eq!(done.rows[0]["secondFactor"], "required");
    assert_eq!(levels.tenant(&space), TenantMode::Required);
    assert_eq!(
        on.store.get(&space).await.expect("read").second_factor,
        TenantMode::Required
    );
    assert!(on
        .execute(
            SET_ADMISSION,
            &space,
            &bound(json!({"secondFactor": "sometimes"})),
            "mem_o"
        )
        .await
        .is_err());
}

// ══════════════════════════ T6 · managing factors ═════════════════════════

use ordo_host::recovery::{Open, Path as RecoveryPath};

/// Sign in with the password and answer the second step with the code for `at`:
/// the token of a session that took it.
async fn password_and_code(app: &axum::Router, secret: &[u8], at: u64) -> String {
    let (_, half) = password_first_step(app).await;
    let (status, done) = answer_totp(app, &half, &totp::totp_at(secret, at, 6)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done["token"].as_str().expect("token").to_string()
}

/// **A person a level makes enrol is made to before a token is minted.** The
/// first step answers no token and says to enrol. The handle and binder
/// authorise the enrolment routes for that person and nothing else, and the
/// second step is then completed with the factor they enrolled. Nobody is
/// signed in without a factor, and nobody is locked out for lacking one.
#[tokio::test(flavor = "multi_thread")]
async fn a_required_person_with_no_factor_is_made_to_enrol_before_a_token_is_minted() {
    let l = layers(SecondFactorConfig {
        mode: DeploymentMode::Required,
        ..SecondFactorConfig::default()
    });
    let app = l.h.app();
    let (status, half) = password_first_step(&app).await;
    assert_eq!(status, StatusCode::OK, "{half}");
    assert!(half.get("token").is_none(), "no token: {half}");
    assert_eq!(half["enrol"], true);
    assert!(half.get("methods").is_none());
    let (handle, binder) = (half["second_step"].clone(), half["binder"].clone());

    // The handle alone does not enrol, nor does the wrong binder.
    let (status, _) = post(
        &app,
        "/auth/factors/totp/begin",
        json!({"second_step": handle}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = post(
        &app,
        "/auth/factors/totp/begin",
        json!({"second_step": handle, "binder": "0".repeat(64)}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // And it is good for enrolling and for nothing else: no bearer, no list.
    let (status, _) = send(&app, "GET", "/auth/factors", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // With both, it enrols a factor.
    let (status, begun) = post(
        &app,
        "/auth/factors/totp/begin",
        json!({"second_step": handle, "binder": binder}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let secret = unbase32(begun["secret"].as_str().expect("secret"));
    let (status, done) = post(
        &app,
        "/auth/factors/totp/confirm",
        json!({"second_step": handle, "binder": binder, "factor": begun["factor"],
               "code": totp::totp_at(&secret, NOW, 6)}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(l.store.all().len(), 1, "the factor is enrolled");

    // Still no token: the second step is completed with the new factor, and the
    // code that proved the enrolment is spent.
    let (status, _) = answer_totp(
        &app,
        &json!({"second_step": handle, "binder": binder}),
        &totp::totp_at(&secret, NOW, 6),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the enrolment's own code");
    l.h.set_now(NOW + 30);
    let (status, done) = answer_totp(
        &app,
        &json!({"second_step": handle, "binder": binder}),
        &totp::totp_at(&secret, NOW + 30, 6),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["second_step"], "totp");
    let claims = claims_of(done["token"].as_str().expect("token"), NOW + 30);
    assert_eq!(
        claims.session.and_then(|s| s.second_step),
        Some(SecondStepKind::Totp)
    );
    // The handle is spent: it enrols nothing more.
    let (status, _) = post(
        &app,
        "/auth/factors/totp/begin",
        json!({"second_step": handle, "binder": binder}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// **Enrolling a factor needs a fresh first factor, and a bearer token alone
/// is refused.** A stolen token cannot enrol a factor, which is the attack the
/// directory's link rules were written against. The proof is a reauth handle
/// (one act's worth, bound to the person) or a passkey assertion that is still
/// fresh, and a confirmation is bound to the person who began.
#[tokio::test(flavor = "multi_thread")]
async fn enrolling_a_factor_needs_a_fresh_first_factor_and_a_bearer_token_alone_is_refused() {
    let l = layers(SecondFactorConfig::default());
    let app = l.h.app();
    let token = token_by_password(&app).await;

    // A bearer alone: refused, and told what to do.
    let (status, body) = send(
        &app,
        "POST",
        "/auth/factors/totp/begin",
        Some(json!({})),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "reauthentication_required");
    assert!(l.store.all().is_empty());

    // A fresh first factor, spent once.
    let handle = reauth_handle(&app, &token, None, NOW).await;
    let (status, begun) = send_with(
        &app,
        "POST",
        "/auth/factors/totp/begin",
        Some(json!({})),
        Some(&token),
        &[("x-reauth", &handle)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let (status, _) = send_with(
        &app,
        "POST",
        "/auth/factors/totp/begin",
        Some(json!({})),
        Some(&token),
        &[("x-reauth", &handle)],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "one act's worth");
    // A handle with no bearer is nothing.
    let (status, _) = send_with(
        &app,
        "POST",
        "/auth/factors/totp/begin",
        Some(json!({})),
        None,
        &[("x-reauth", &handle)],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A confirmation is bound to the person who began: another person's
    // session cannot finish it, and cannot spend it either.
    let device = TestAuthenticator::es256();
    let (_, options) = post(&app, "/auth/register", json!({"method": "passkey"})).await;
    let (status, signed) = post(
        &app,
        "/auth/complete",
        registration_body(&device.register(options["challenge"].as_str().expect("challenge"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{signed}");
    let bea = signed["token"].as_str().expect("token").to_string();
    let secret = unbase32(begun["secret"].as_str().expect("secret"));
    let confirm = json!({"factor": begun["factor"], "code": totp::totp_at(&secret, NOW, 6)});
    let (status, _) = send(
        &app,
        "POST",
        "/auth/factors/totp/confirm",
        Some(confirm.clone()),
        Some(&bea),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "not hers to confirm");
    assert!(l.store.all().is_empty());
    let (status, done) = send(
        &app,
        "POST",
        "/auth/factors/totp/confirm",
        Some(confirm),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "still theirs to confirm: {done}");

    // Presence is a fresh first factor too, for as long as it is fresh.
    let (status, _) = send(
        &app,
        "POST",
        "/auth/factors/totp/begin",
        Some(json!({})),
        Some(&bea),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a fresh passkey assertion needs no handle"
    );
    l.h.set_now(NOW + 301);
    let (status, _) = send(
        &app,
        "POST",
        "/auth/factors/totp/begin",
        Some(json!({})),
        Some(&bea),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "presence went stale");

    // A half-signed-in state that is asking for a second step, and not for an
    // enrolment, authorises no enrolment: the person holds a factor by now.
    let (_, half) = password_first_step(&app).await;
    assert!(
        half.get("enrol").is_none() && half["methods"] == json!(["totp"]),
        "{half}"
    );
    let (status, _) = post(
        &app,
        "/auth/factors/totp/begin",
        json!({"second_step": half["second_step"], "binder": half["binder"]}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a step is not an enrolment"
    );
}

/// **Removing a factor ends the other sessions.** It needs the proof a change
/// needs, with the second step behind it, and it hands the person who proved
/// themselves a replacement session so they are not signed out for it. A
/// factor that is not theirs is the one refusal and ends nothing.
#[tokio::test(flavor = "multi_thread")]
async fn removing_a_factor_ends_the_other_sessions() {
    let l = layers(SecondFactorConfig::default());
    let app = l.h.app();
    let secret = enrol_totp(&app, &token_by_password(&app).await, NOW).await;
    let factor = l.store.all()[0].id.clone();

    // Two sessions the person has, each of which took the second step.
    l.h.set_now(NOW + 30);
    let phone = password_and_code(&app, &secret, NOW + 30).await;
    l.h.set_now(NOW + 60);
    let laptop = password_and_code(&app, &secret, NOW + 60).await;
    let ana = claims_of(&laptop, NOW + 60).principal;
    let path = format!("/auth/factors/{factor}/remove");

    // Without proof: refused, and nothing is removed.
    let (status, body) = send(&app, "POST", &path, None, Some(&laptop)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "reauthentication_required");
    assert_eq!(l.store.all().len(), 1);

    // A factor that is not theirs, with proof: the one refusal, and no session
    // ends.
    l.h.set_now(NOW + 90);
    let handle = reauth_handle(&app, &laptop, Some(&secret), NOW + 90).await;
    let (status, _) = send_with(
        &app,
        "POST",
        "/auth/factors/sf_nobody/remove",
        None,
        Some(&laptop),
        &[("x-reauth", &handle)],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let mut door = door_over(&ana, &l.levels, NOW + 91);
    door.revocations = Some(l.h.revocations.clone());
    assert!(allowed(&door, &phone, SPACE), "nothing ended");

    // With proof, it is removed and every session ends but the one handed back.
    l.h.set_now(NOW + 120);
    let handle = reauth_handle(&app, &laptop, Some(&secret), NOW + 120).await;
    let (status, done) = send_with(
        &app,
        "POST",
        &path,
        None,
        Some(&laptop),
        &[("x-reauth", &handle)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["removed"], factor.as_str());
    let kept = done["token"]
        .as_str()
        .expect("a replacement session")
        .to_string();
    assert!(l.store.all().is_empty());

    l.h.set_now(NOW + 122);
    let mut door = door_over(&ana, &l.levels, NOW + 122);
    door.revocations = Some(l.h.revocations.clone());
    assert!(!allowed(&door, &phone, SPACE), "the other device is ended");
    assert!(!allowed(&door, &laptop, SPACE), "and so is the old token");
    assert!(
        allowed(&door, &kept, SPACE),
        "the person who proved themselves stays"
    );
    let kinds = reasons(&drained(&l.h));
    assert!(
        kinds
            .iter()
            .any(|k| k.starts_with("second-factor removed") && k.contains("by=self")),
        "{kinds:?}"
    );
    assert!(
        kinds.iter().any(|k| k.contains("second-factor-removed")),
        "the sessions ending is recorded: {kinds:?}"
    );

    // With the factor gone the password alone signs in again.
    let (status, signed) = password_first_step(&app).await;
    assert_eq!(status, StatusCode::OK, "{signed}");
    assert!(signed["token"].as_str().is_some(), "{signed}");
}

/// A person reads their own factors, and sets their own level, with the proof a
/// change needs. Neither shows a secret, and the level is recorded.
#[tokio::test(flavor = "multi_thread")]
async fn a_person_lists_their_factors_and_sets_their_own_level() {
    let l = layers(SecondFactorConfig::default());
    let app = l.h.app();
    let token = token_by_password(&app).await;
    let secret = enrol_totp(&app, &token, NOW).await;

    let (status, list) = send(&app, "GET", "/auth/factors", None, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["factors"][0]["kind"], "totp");
    assert_eq!(list["factors"][0]["label"], "phone");
    assert_eq!(list["factors"][0]["locked"], false);
    assert_eq!(list["require_second_step"], "when-enrolled");
    assert_eq!(list["second_step"], "enabled");
    assert!(
        !list.to_string().contains(&totp::base32(&secret)),
        "no secret is listed"
    );
    let (status, _) = send(&app, "GET", "/auth/factors", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Setting the level needs the proof, and a word it knows.
    l.h.set_now(NOW + 30);
    let body = json!({"require_second_step": "also-for-passkeys"});
    let (status, _) = send(
        &app,
        "POST",
        "/auth/factors/policy",
        Some(body.clone()),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let handle = reauth_handle(&app, &token, Some(&secret), NOW + 30).await;
    let (status, _) = send_with(
        &app,
        "POST",
        "/auth/factors/policy",
        Some(json!({"require_second_step": "sometimes"})),
        Some(&token),
        &[("x-reauth", &handle)],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "not a level");
    l.h.set_now(NOW + 60);
    let handle = reauth_handle(&app, &token, Some(&secret), NOW + 60).await;
    let (status, done) = send_with(
        &app,
        "POST",
        "/auth/factors/policy",
        Some(body),
        Some(&token),
        &[("x-reauth", &handle)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let ana = claims_of(&token, NOW).principal.0;
    assert_eq!(
        l.store.person_mode(&ana).expect("read"),
        PersonMode::AlsoForPasskeys
    );
    let kinds = reasons(&drained(&l.h));
    assert!(
        kinds
            .iter()
            .any(|k| k.starts_with("second-factor policy-changed")
                && k.contains("also-for-passkeys")),
        "{kinds:?}"
    );
}

/// **A recovery removes every second factor once it completes**, and not when
/// it is opened (owner, 2026-09-29: *"but only after the reset"*). A case that
/// is cancelled removes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_recovery_removes_every_second_factor_only_once_it_completes() {
    let f = full_harness(SecondFactorConfig::default());
    let app = f.h.app();
    let token = token_by_password(&app).await;
    enrol_totp(&app, &token, NOW).await;
    let ana = claims_of(&token, NOW).principal.0;
    assert_eq!(f.store.all().len(), 1);
    f.h.set_now(NOW + 30);
    let open = || Open {
        principal: &ana,
        path: RecoveryPath::Host,
        opened_by: "adam",
        authority: "host",
        evidence: "host-break-glass",
        reference: "TICKET-1",
        space: "",
        extra_delay_secs: 0,
        approvals: 0,
    };

    // A case that is opened and cancelled removes nothing.
    let opened = f.recovery.open(&open()).expect("opened");
    assert_eq!(f.store.all().len(), 1, "opening removes nothing");
    f.recovery
        .cancel(
            &opened.case,
            ordo_secevent::Subject::member(&ana),
            "changed-mind",
        )
        .expect("cancelled");
    assert_eq!(f.store.all().len(), 1, "a cancelled case removes nothing");

    // One that completes does.
    let ticket = f
        .recovery
        .open(&open())
        .expect("opened")
        .ticket
        .expect("a host recovery matures at once");
    assert_eq!(f.store.all().len(), 1, "still there until it completes");
    let device = TestAuthenticator::es256();
    let (status, options) = post(&app, "/auth/recover/begin", json!({"ticket": ticket})).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let mut body =
        registration_body(&device.register(options["challenge"].as_str().expect("challenge")));
    body["ticket"] = json!(ticket);
    let (status, done) = post(&app, "/auth/recover/complete", body).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert!(
        f.store.all().is_empty(),
        "the reset is complete: {:?}",
        f.store.all()
    );
    let kinds = reasons(&drained(&f.h));
    assert!(
        kinds
            .iter()
            .any(|k| k.starts_with("second-factor removed") && k.contains("by=recovery")),
        "{kinds:?}"
    );
}
