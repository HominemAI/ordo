//! The guessing slowdown on password sign-in (owner, 2026-10-25,
//! `guessing-a-password-or-a-link`), through `/auth/begin` and
//! `/auth/complete`.
//!
//! Before it, 200 wrong passwords against one account went through with no
//! delay. Now, after 5 wrong in a row, each further try waits a doubling
//! delay up to 60 s, answered `429 too_many_attempts` with `Retry-After`.
//! The count is per account, in the control database, shared by every node.
//! The app developer and a tenant may change it, inside their bounds.
//!
//! The Postgres tests need the conformance cluster
//! (`scripts/conformance-pg.sh start`); databases are prefixed
//! `ordo_guess_`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::BoundParams;
use ordo_host::auth::AuthState;
use ordo_host::guess_limits::{Deployment, GuessLimits, SET_APP, SET_TENANT};
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::provider::{Argon2idHasher, LocalPasswordProvider};
use ordo_identity::{MemoryCredentialStore, MemorySessionRevocations, ProviderRegistry, TokenKey};
use ordo_secevent::{EventKind, RecordedDecision, Recorder, Scope};
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &[u8] = b"guess-limits-test-secret";
const NOW: u64 = 1_000_000;
const ANA: &str = "ana@example.test";
const RIGHT: &str = "correct-horse";
const HOME: &str = "home";

/// Tests that each make a database take turns.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn store() -> Arc<MemoryCredentialStore> {
    let store = Arc::new(MemoryCredentialStore::new());
    LocalPasswordProvider::with_store(Argon2idHasher::weak_for_tests(), store.clone())
        .enroll(ANA, RIGHT)
        .expect("enrolled");
    store
}

/// One door: a password provider over `credentials`, the slowdown `guess`
/// (or the built-in one), its clock at `at`.
fn door(
    credentials: Arc<MemoryCredentialStore>,
    guess: Option<Arc<GuessLimits>>,
    recorder: Arc<Recorder>,
    at: u64,
) -> axum::Router {
    let provider = LocalPasswordProvider::with_store(Argon2idHasher::weak_for_tests(), credentials);
    let mut registry = ProviderRegistry::new();
    registry
        .register("password", Box::new(provider))
        .expect("one provider");
    let mut state = AuthState::new(
        registry,
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_recorder(recorder);
    if let Some(g) = guess {
        state = state.with_guess_limits(g);
    }
    state.now = Box::new(move || at);
    ordo_host::auth::router(Some(Arc::new(state)))
}

async fn post(app: &axum::Router, path: &str, body: Value) -> (StatusCode, Option<u64>, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let wait = res
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        wait,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// One password sign-in, begun and completed, optionally naming a tenant.
async fn try_password(
    app: &axum::Router,
    who: &str,
    password: &str,
    space: Option<&str>,
) -> (StatusCode, Option<u64>, Value) {
    let (st, _, begun) = post(
        app,
        "/auth/begin",
        json!({"method": "password", "identifier": who}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{begun}");
    let mut body = json!({
        "method": "password",
        "kind": "password",
        "display": "Ana",
        "challenge": begun["challenge"],
        "identifier": who,
        "password": password,
    });
    if let Some(s) = space {
        body["space"] = json!(s);
    }
    post(app, "/auth/complete", body).await
}

/// The `Throttled` entries on the stream: (scope, count).
fn throttled(recorder: &Recorder) -> Vec<(String, u64)> {
    recorder
        .drain_all(ordo_commandlog::Timestamp(NOW + 10))
        .into_iter()
        .filter_map(|e| match &e.kind {
            EventKind::Denied(d) if d.contains(&RecordedDecision::Throttled) => Some((
                match e.scope {
                    Scope::Deployment => "deployment".to_string(),
                    Scope::Space(s) => format!("space:{}", s.0),
                },
                e.count,
            )),
            _ => None,
        })
        .collect()
}

async fn pool(db: &str) -> ordo_exec::Pool {
    let c = ordo_exec::PoolConfig::from_env();
    ordo_exec::drop_database(&c, db).await.expect("drop");
    ordo_exec::create_database(&c, db).await.expect("create");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 4,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    ordo_host::guess_limits::ensure_table(&pool)
        .await
        .expect("tables");
    pool
}

fn shared(pool: &ordo_exec::Pool, recorder: &Arc<Recorder>) -> Arc<GuessLimits> {
    Arc::new(
        GuessLimits::on_pool(Deployment::default(), pool.clone())
            .with_home(HOME)
            .with_stream(recorder.clone()),
    )
}

fn params(pairs: &[(&str, &str)]) -> BoundParams {
    BoundParams(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect(),
    )
}

/// The confirmed defect, closed: 30 wrong passwords. The first 5 are the one
/// refusal; every one after waits, with `Retry-After` and the setting that
/// changes it, and is one collapsed `Throttled` entry. The right password
/// waits its turn too, then signs in, and the count starts again.
#[tokio::test(flavor = "multi_thread")]
async fn thirty_wrong_passwords_are_slowed_and_the_right_one_works_after_the_wait() {
    let _serial = SERIAL.lock().await;
    let pool = pool("ordo_guess_thirty").await;
    let recorder = Arc::new(Recorder::default());
    let guess = shared(&pool, &recorder);
    let credentials = store();
    let app = door(
        credentials.clone(),
        Some(guess.clone()),
        recorder.clone(),
        NOW,
    );
    for i in 0..30 {
        let (st, wait, b) = try_password(&app, ANA, "wrong", None).await;
        if i < 5 {
            assert_eq!(st, StatusCode::UNAUTHORIZED, "try {i}: {b}");
        } else {
            assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "try {i}: {b}");
            assert_eq!(b["error"]["code"], "too_many_attempts");
            assert_eq!(wait, Some(1), "try {i}");
            assert_eq!(b["error"]["retry_after"], 1);
            assert_eq!(b["error"]["setting"], SET_APP);
        }
    }
    let (st, _, _) = try_password(&app, ANA, RIGHT, None).await;
    assert_eq!(
        st,
        StatusCode::TOO_MANY_REQUESTS,
        "the right one waits its turn"
    );
    assert_eq!(
        throttled(&recorder),
        [("deployment".to_string(), 26)],
        "one entry for the burst"
    );

    let later = door(credentials, Some(guess), recorder, NOW + 1);
    let (st, _, b) = try_password(&later, ANA, RIGHT, None).await;
    assert_eq!(st, StatusCode::OK, "after the wait: {b}");
    assert!(b["token"].is_string());
    for i in 0..5 {
        let (st, _, b) = try_password(&later, ANA, "wrong", None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "reset, try {i}: {b}");
    }
}

/// An identifier nobody holds is slowed exactly like a real one, so the
/// slowdown tells a stranger nothing about which accounts exist. Another
/// account is not slowed by it.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_account_is_slowed_the_same_and_others_are_not() {
    let _serial = SERIAL.lock().await;
    let pool = pool("ordo_guess_unknown").await;
    let recorder = Arc::new(Recorder::default());
    let app = door(store(), Some(shared(&pool, &recorder)), recorder, NOW);
    for who in ["nobody@example.test", ANA] {
        for _ in 0..5 {
            let (st, _, _) = try_password(&app, who, "wrong", None).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED);
        }
        let (st, wait, b) = try_password(&app, who, "wrong", None).await;
        assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{who}: {b}");
        assert_eq!(wait, Some(1));
    }
}

/// Two doors over one database are two nodes: the count is one.
#[tokio::test(flavor = "multi_thread")]
async fn the_count_is_shared_between_two_doors_on_one_database() {
    let _serial = SERIAL.lock().await;
    let pool = pool("ordo_guess_two").await;
    let credentials = store();
    let (ra, rb) = (Arc::new(Recorder::default()), Arc::new(Recorder::default()));
    let a = door(credentials.clone(), Some(shared(&pool, &ra)), ra, NOW);
    let b = door(credentials, Some(shared(&pool, &rb)), rb, NOW);
    for i in 0..5 {
        let app = if i % 2 == 0 { &a } else { &b };
        let (st, _, _) = try_password(app, ANA, "wrong", None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }
    for app in [&a, &b] {
        let (st, wait, body) = try_password(app, ANA, "wrong", None).await;
        assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{body}");
        assert_eq!(wait, Some(1));
    }
}

/// The app developer moves the threshold and the cap, or turns it off. A
/// tenant moves it for sign-ins that name it, inside the app developer's
/// range, and turns it off only where that range starts at 0. Each change
/// is on the record.
#[tokio::test(flavor = "multi_thread")]
async fn the_app_developer_and_a_tenant_change_it_inside_their_bounds() {
    let _serial = SERIAL.lock().await;
    let pool = pool("ordo_guess_layers").await;
    let recorder = Arc::new(Recorder::default());
    let guess = shared(&pool, &recorder);
    let credentials = store();
    let mut at = NOW;
    let mut round = |space: Option<&'static str>, n: usize| {
        at += 1_000;
        let app = door(
            credentials.clone(),
            Some(guess.clone()),
            recorder.clone(),
            at,
        );
        async move {
            // The right one, past any wait, starts the count again.
            let (st, _, b) = try_password(&app, ANA, RIGHT, None).await;
            assert_eq!(st, StatusCode::OK, "{b}");
            let mut codes = Vec::new();
            for _ in 0..n {
                let (st, _, _) = try_password(&app, ANA, "wrong", space).await;
                codes.push(st.as_u16());
            }
            codes
        }
    };

    // The app developer: slow after 2, wait at most 30 s.
    let out = guess
        .execute(
            SET_APP,
            HOME,
            &params(&[("slowAfter", "2"), ("slowCapSecs", "30")]),
            "own_1",
        )
        .await
        .expect("the app developer's layer")
        .rows;
    assert_eq!(out[0]["slowAfter"], 2);
    assert_eq!(out[0]["slowAfterFrom"], "app");
    assert_eq!(out[0]["slowCapSecs"], 30);
    assert_eq!(round(None, 3).await, [401, 401, 429]);
    // Only in the deployment's own space.
    let e = guess
        .execute(SET_APP, "t1", &params(&[("slowAfter", "3")]), "own_1")
        .await
        .expect_err("not the home space");
    assert_eq!(e.code, ordo_api::ErrorCode::NotFound);
    // Outside the deployment's limits names the limit.
    let e = guess
        .execute(SET_APP, HOME, &params(&[("slowAfter", "500")]), "own_1")
        .await
        .expect_err("past the limit");
    assert!(
        e.to_string().contains("ORDO_PASSWORD_SLOW_AFTER_LIMIT"),
        "{e}"
    );

    // A tenant with its own sign-in requirements: 6, for sign-ins naming it.
    let out = guess
        .execute(SET_TENANT, "t1", &params(&[("slowAfter", "6")]), "own_t1")
        .await
        .expect("a tenant's own")
        .rows;
    assert_eq!(out[0]["slowAfter"], 6);
    assert_eq!(out[0]["slowAfterFrom"], "tenant");
    assert_eq!(
        round(Some("t1"), 7).await,
        [401, 401, 401, 401, 401, 401, 429]
    );
    // A sign-in that names no tenant keeps the app developer's 2.
    assert_eq!(round(None, 3).await, [401, 401, 429]);

    // Off is refused to the tenant, naming the app developer's setting.
    let e = guess
        .execute(SET_TENANT, "t1", &params(&[("slowAfter", "off")]), "own_t1")
        .await
        .expect_err("off is the app developer's to allow");
    assert!(e.to_string().contains("slowAfterRange"), "{e}");
    assert!(e.to_string().contains(SET_APP), "{e}");
    // The app developer narrows the tenants' range: the stored 6 is held.
    guess
        .execute(
            SET_APP,
            HOME,
            &params(&[("slowAfterRange", "1-4")]),
            "own_1",
        )
        .await
        .expect("a range");
    let e = guess
        .execute(SET_TENANT, "t1", &params(&[("slowAfter", "6")]), "own_t1")
        .await
        .expect_err("outside the app developer's range");
    assert!(
        e.to_string().contains("password_slowdown_out_of_range"),
        "{e}"
    );
    assert_eq!(round(Some("t1"), 5).await, [401, 401, 401, 401, 429]);

    // The app developer turns it off for everyone it governs.
    guess
        .execute(
            SET_APP,
            HOME,
            &params(&[("slowAfter", "off"), ("slowAfterRange", "none")]),
            "own_1",
        )
        .await
        .expect("off");
    guess
        .execute(
            SET_TENANT,
            "t1",
            &params(&[("slowAfter", "default")]),
            "own_t1",
        )
        .await
        .expect("default");
    assert!(round(None, 12).await.iter().all(|c| *c == 401));
    assert!(round(Some("t1"), 12).await.iter().all(|c| *c == 401));

    // Every change is on the record: the tenant's on the tenant's stream.
    let acts: Vec<String> = recorder
        .drain_all(ordo_commandlog::Timestamp(at + 10))
        .into_iter()
        .filter_map(|e| match (&e.kind, &e.scope) {
            (EventKind::Enrolment { act, .. }, Scope::Space(s))
                if act == "password-slowdown-set" =>
            {
                Some(s.0.clone())
            }
            _ => None,
        })
        .collect();
    assert!(acts.iter().any(|s| s == "t1"), "{acts:?}");
}

/// Never off by omission: a door given no slowdown of its own has Ordo's
/// default, in memory.
#[tokio::test(flavor = "multi_thread")]
async fn a_door_wired_with_nothing_still_slows() {
    let app = door(store(), None, Arc::new(Recorder::default()), NOW);
    for _ in 0..5 {
        let (st, _, _) = try_password(&app, ANA, "wrong", None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }
    let (st, wait, _) = try_password(&app, ANA, "wrong", None).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(wait, Some(1));
}
