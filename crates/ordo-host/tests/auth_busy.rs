//! A saturated control pool answers the sign-in routes the way it answers
//! ops: a typed, retryable `503 server_busy` with `Retry-After`. Any other
//! store failure stays `503 credential_store_unavailable`.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::auth::AuthState;
use ordo_host::email_code::{
    self, EmailCodePolicy, EmailCodeProvider, PgEmailCodeStore, Registered,
};
use ordo_host::mail::{self, PgMailQueue};
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::{MemoryDirectoryStore, MemorySessionRevocations, ProviderRegistry, TokenKey};
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &[u8] = b"auth-busy-test-secret-of-32-bytes!!";

async fn app(stem: &str, size: usize) -> (axum::Router, Pool) {
    let pg = ordo_conformance::Pg::scoped(stem);
    let f = ordo_conformance::Fixture::load().expect("fixture");
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size,
        acquire_timeout: Duration::from_millis(300),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");
    ordo_host::ceremonies::ensure_table(&pool)
        .await
        .expect("ceremonies");
    mail::ensure_table(&pool).await.expect("outbox");
    email_code::ensure_table(&pool).await.expect("rates");
    let dir = Arc::new(MemoryDirectoryStore::new());
    let email = Arc::new(EmailCodeProvider::new(
        SECRET,
        EmailCodePolicy::default(),
        Arc::new(PgEmailCodeStore::open_on(pool.clone()).expect("store")),
        Arc::new(PgMailQueue::open_on(pool.clone()).expect("queue")),
        dir.clone(),
        Provisioning::AutoOnboard,
    ));
    let mut registry = ProviderRegistry::new();
    registry
        .register("email", Box::new(Registered(email.clone())))
        .expect("registered");
    let state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, dir),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_email(email);
    (ordo_host::auth::router(Some(Arc::new(state))), pool)
}

async fn begin(app: &axum::Router) -> (StatusCode, Option<String>, Value) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/begin")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"method": "email", "identifier": "kim@example.org"}).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = res.status();
    let retry = res
        .headers()
        .get("retry-after")
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        retry,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_pool_answers_email_begin_as_server_busy() {
    let (app, pool) = app("auth_busy_full", 1).await;
    let held = pool.begin().await.expect("the one connection");
    let (status, retry, body) = begin(&app).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "server_busy", "{body}");
    assert_eq!(retry.as_deref(), Some("1"));
    drop(held);
    let (status, _, body) = begin(&app).await;
    assert_eq!(status, StatusCode::OK, "served again once free: {body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_store_stays_credential_store_unavailable() {
    let (app, pool) = app("auth_busy_broken", 2).await;
    pool.rows(r#"DROP TABLE "$authceremony""#)
        .await
        .expect("dropped");
    let (status, retry, body) = begin(&app).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(
        body["error"]["code"], "credential_store_unavailable",
        "{body}"
    );
    assert_eq!(retry, None);
}

// ---------------------------------------------------------------------------
// The other sign-in paths, each under a held pool: passkey, guest, claim,
// recovery and link. Every one answers `server_busy` through the one
// interface (`StoreFailure`), and is served again once the pool is free.

/// A deployment whose stores all live on `pool`: passkey keys and
/// ceremonies, the guest setting, recovery's tickets and the share links.
async fn every_store(stem: &str) -> (axum::Router, Pool) {
    let pg = ordo_conformance::Pg::scoped(stem);
    let f = ordo_conformance::Fixture::load().expect("fixture");
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 1,
        acquire_timeout: Duration::from_millis(300),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");
    ordo_host::ceremonies::ensure_table(&pool)
        .await
        .expect("ceremonies");
    ordo_host::passkeys::ensure_table(&pool)
        .await
        .expect("passkeys");
    ordo_host::admission::ensure_table(&pool)
        .await
        .expect("settings");
    ordo_host::tickets::ensure_table(&pool)
        .await
        .expect("tickets");
    ordo_host::share_links::ensure_table(&pool)
        .await
        .expect("links");
    let ceremonies = Arc::new(
        ordo_host::ceremonies::PgCeremonyStore::open_on(pool.clone()).expect("ceremony store"),
    );
    let passkey = Arc::new(ordo_identity::PasskeyProvider::with_stores(
        ordo_identity::RelyingParty::new("example.com", &["https://app.example.com"])
            .expect("a relying party"),
        Arc::new(ordo_host::passkeys::PgPasskeyStore::open_on(pool.clone()).expect("keys")),
        ceremonies.clone(),
    ));
    let dir = Arc::new(MemoryDirectoryStore::new());
    let mut registry = ProviderRegistry::new();
    registry
        .register("passkey", Box::new(passkey.clone()))
        .expect("registered");
    registry
        .register(
            ordo_host::auth::guest::METHOD,
            Box::new(ordo_host::auth::guest::GuestProvider::new(dir.clone())),
        )
        .expect("registered");
    let state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, dir),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_passkey(passkey)
    .with_ceremonies(ceremonies)
    .with_guests(ordo_host::auth::guest::Guests::stored(
        ordo_host::auth::guest::GuestStore::new(pool.clone()),
    ));
    let tickets = Arc::new(ordo_host::tickets::Tickets::new(Arc::new(
        ordo_host::tickets::PgTicketStore::open_on(pool.clone()).expect("ticket store"),
    )));
    let recovery = Arc::new(ordo_host::recovery::Recovery::new(
        Arc::new(ordo_host::recovery::MemoryRecoveryStore::new()),
        tickets.clone(),
        ordo_host::recovery::RecoveryPolicy::default(),
    ));
    let share = Arc::new(ordo_host::share_ops::ShareOps {
        tickets,
        stream: None,
        now: Arc::new(|| 1_800_000_000),
        max_secs: None,
        grants: Arc::new(ordo_host::grants::PgGrantRegistry::new(pool.clone())),
        members: Arc::new(ordo_host::member_ops::MemberOps {
            control: pool.clone(),
            pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
            directory: None,
            stream: None,
            refresh: None,
            now: Arc::new(|| 1_800_000_000),
            managed: None,
        }),
        accept: None,
    });
    let links = Arc::new(ordo_host::share_links::ShareLinks::new(
        share,
        ordo_host::share_links::LinkSettings::default(),
    ));
    let state = Arc::new(state);
    state.wire_recovery(recovery);
    state.wire_links(links);
    (ordo_host::auth::router(Some(state)), pool)
}

/// A live session's bearer, for the routes that need one.
fn session_token() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &ordo_identity::TokenSpec {
            principal: ordo_identity::PrincipalId("usr_kim".into()),
            expires: ordo_identity::Timestamp(now + 3600),
            client: None,
            user_presence_at: Some(ordo_identity::Timestamp(now)),
            capability: None,
            delegation: None,
            session: Some(ordo_identity::session::SessionClaim {
                id: ordo_identity::session::SessionId::mint(),
                started_at: ordo_identity::Timestamp(now),
                second_step: None,
                via: None,
                upstream_presence: None,
            }),
        },
    )
}

async fn post(
    app: &axum::Router,
    path: &str,
    body: Value,
    bearer: Option<&str>,
) -> (StatusCode, Option<String>, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(b) = bearer {
        req = req.header("authorization", format!("Bearer {b}"));
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let retry = res
        .headers()
        .get("retry-after")
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        retry,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Held: `server_busy` with `Retry-After`. Free again: anything but busy.
async fn busy_then_served(stem: &str, path: &str, body: Value, bearer: Option<&str>) {
    let (app, pool) = every_store(stem).await;
    let held = pool.begin().await.expect("the one connection");
    let (status, retry, answer) = post(&app, path, body.clone(), bearer).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}: {answer}");
    assert_eq!(answer["error"]["code"], "server_busy", "{path}: {answer}");
    assert_eq!(retry.as_deref(), Some("1"), "{path}");
    drop(held);
    let (status, _, answer) = post(&app, path, body, bearer).await;
    assert_ne!(
        answer["error"]["code"], "server_busy",
        "{path} once free: {status} {answer}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_pool_answers_passkey_begin_as_server_busy() {
    busy_then_served(
        "auth_busy_passkey",
        "/auth/begin",
        json!({"method": "passkey"}),
        None,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_pool_answers_a_new_guest_as_server_busy() {
    busy_then_served(
        "auth_busy_guest",
        "/auth/complete",
        json!({"method": "guest", "kind": "new"}),
        None,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_pool_answers_a_claim_as_server_busy() {
    let token = session_token();
    busy_then_served(
        "auth_busy_claim",
        "/auth/claim/complete",
        json!({"claim": "clm_not_a_real_one"}),
        Some(&token),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_pool_answers_recovery_begin_as_server_busy() {
    busy_then_served(
        "auth_busy_recover",
        "/auth/recover/begin",
        // Well formed, so the store is asked.
        json!({"ticket": format!("{}{}", ordo_host::tickets::PREFIX, "b".repeat(64))}),
        None,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_pool_answers_a_link_as_server_busy() {
    let secret = format!(
        "{}{}",
        ordo_host::share_links::SECRET_PREFIX,
        "a".repeat(64)
    );
    busy_then_served(
        "auth_busy_link",
        "/auth/link",
        json!({"link": secret}),
        None,
    )
    .await;
}
