//! The door verifies the identity service's tokens (`plan-identity-service.md`
//! 4.2, 4.3, step 3), against an in-process fake issuer: a signer and a JWKS
//! route on a real socket.
//!
//! Each row of the realm table, a stale touch, an expired token, the wrong
//! `aud` and `iss`, an unknown `kid` then a fetch, the service down with
//! stored keys, a rotation, the `/auth/*` refusal, the person's `sub` in the
//! security record, and the `$identitykeys` table on Postgres.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::SigningKey;
use ordo_api::{AccessChecker, AccessDecision};
use ordo_host::identity_keys::{
    IdentityKeyStore, IdentityKeys, IdentitySettings, ServiceUrl, UNKNOWN_KID_GAP_SECS,
};
use ordo_host::metrics::Metrics;
use ordo_host::IdentityAccess;
use ordo_identity::deployment_token::{self, DeploymentClaims, Kind, ROLE_ADMIN};
use ordo_identity::{AccessEngine, AccessProfile, PrincipalId, Role, SpaceId, TokenKey};
use ordo_secevent::{EventKind, RecordedDecision};
use serde_json::{json, Value};
use tower::ServiceExt;

const NOW: u64 = 2_000_000;
const ISS: &str = "https://console.example.com:8082";
const HOME: &str = "system";
const TENANT: &str = "tenant-a";
/// A platform write the presence gate names.
const APPLY: &str = "std.deployment.apply";
/// A platform read it does not.
const COST: &str = "std.deployment.observedCost";
/// A tenant op on the open list.
const MEMBERS: &str = ordo_host::people_ops::MEMBERS;
/// An app op.
const APP_OP: &str = "notes.create";
const SECRET: &[u8] = b"identity-tokens-test-secret";

/// The fake identity service: its keys, and what its JWKS route answers.
#[derive(Clone)]
struct Issuer {
    /// `None` answers 503: the service is up but not serving keys.
    published: Arc<Mutex<Option<Value>>>,
    hits: Arc<AtomicUsize>,
    url: String,
}

impl Issuer {
    async fn start() -> Issuer {
        let published: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
        let hits = Arc::new(AtomicUsize::new(0));
        let (p, h) = (published.clone(), hits.clone());
        let app = axum::Router::new().route(
            ordo_host::identity_keys::JWKS_PATH,
            axum::routing::get(move || {
                let (p, h) = (p.clone(), h.clone());
                async move {
                    h.fetch_add(1, Ordering::SeqCst);
                    match p.lock().unwrap().clone() {
                        Some(v) => (StatusCode::OK, v.to_string()),
                        None => (StatusCode::SERVICE_UNAVAILABLE, String::new()),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Issuer {
            published,
            hits,
            url,
        }
    }

    fn publish(&self, keys: &[&SigningKey]) {
        let keys: Vec<Value> = keys
            .iter()
            .map(|k| deployment_token::public_jwk(&kid(k), &k.verifying_key()))
            .collect();
        *self.published.lock().unwrap() = Some(json!({ "keys": keys }));
    }

    fn down(&self) {
        *self.published.lock().unwrap() = None;
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

fn kid(k: &SigningKey) -> String {
    deployment_token::key_id(&k.verifying_key())
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn settings(url: &str) -> IdentitySettings {
    IdentitySettings {
        url: ServiceUrl::parse(url, false).unwrap(),
        issuer: ISS.into(),
        audience: deployment_token::audience("main"),
    }
}

fn claims(kind: Kind, touched_at: Option<u64>) -> DeploymentClaims {
    let sub = match kind {
        Kind::Person => "p_ada",
        Kind::Machine => "m_ci",
        Kind::Breakglass => "breakglass:root",
    };
    DeploymentClaims {
        iss: ISS.into(),
        aud: deployment_token::audience("main"),
        sub: sub.into(),
        kind,
        role: ROLE_ADMIN.into(),
        client: "console".into(),
        sid: "s_1".into(),
        touched_at,
        iat: NOW,
        exp: NOW + 600,
        jti: "j_1".into(),
    }
}

fn sign(c: &DeploymentClaims, k: &SigningKey) -> String {
    deployment_token::sign(c, &kid(k), k)
}

struct Door {
    door: IdentityAccess,
    keys: Arc<IdentityKeys>,
    clock: Arc<AtomicU64>,
}

impl Door {
    fn at(&self, t: u64) {
        self.clock.store(t, Ordering::SeqCst);
    }

    /// Each recorded denial as (subject, decision), and each allowed
    /// deployment act as (subject, None).
    fn record(&self) -> Vec<(String, Option<RecordedDecision>)> {
        self.door
            .take_events()
            .into_iter()
            .flat_map(|e| {
                let who = e.subject.render();
                match e.kind {
                    EventKind::Denied(ds) => {
                        ds.into_iter().map(|d| (who.clone(), Some(d))).collect()
                    }
                    EventKind::Authenticated => vec![(who, None)],
                    _ => Vec::new(),
                }
            })
            .collect()
    }
}

/// A door whose engine gates `APPLY` with a touch, and one app owner in
/// the home space, over the identity service at `url`.
async fn door(url: &str, store: IdentityKeyStore) -> Door {
    let clock = Arc::new(AtomicU64::new(NOW));
    let mut engine = AccessEngine::new();
    for space in [HOME, TENANT] {
        let sid = SpaceId(space.into());
        engine.set_profile(
            &sid,
            AccessProfile::new()
                .allow(Role::Owner, APPLY)
                .allow(Role::Owner, COST)
                .allow(Role::Owner, APP_OP),
        );
    }
    engine.set_membership(
        &SpaceId(HOME.into()),
        &PrincipalId("usr_owner".into()),
        Role::Owner,
    );
    engine.require_step_up(APPLY);
    let read = clock.clone();
    let keys = Arc::new(IdentityKeys::new(
        settings(url),
        store,
        Arc::new(move || read.load(Ordering::SeqCst)),
    ));
    let read = clock.clone();
    let door = IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(move || read.load(Ordering::SeqCst)),
        Arc::new(Metrics::new()),
    )
    .with_console_home(HOME)
    .with_identity_keys(keys.clone());
    Door { door, keys, clock }
}

fn allowed(d: AccessDecision) -> Option<String> {
    match d {
        AccessDecision::Allow { principal, .. } => Some(principal),
        AccessDecision::Deny => None,
    }
}

#[tokio::test]
async fn a_person_with_a_fresh_touch_reaches_platform_ops_and_the_open_tenant_list() {
    let issuer = Issuer::start().await;
    let k = key(1);
    issuer.publish(&[&k]);
    let d = door(&issuer.url, IdentityKeyStore::memory()).await;
    d.keys.boot().await.expect("booted");
    let t = sign(&claims(Kind::Person, Some(NOW - 10)), &k);

    assert_eq!(allowed(d.door.check(&t, HOME, APPLY)), Some("p_ada".into()));
    assert_eq!(allowed(d.door.check(&t, HOME, COST)), Some("p_ada".into()));
    assert_eq!(
        allowed(d.door.check(&t, TENANT, MEMBERS)),
        Some("p_ada".into())
    );
    // Not on the open list, an app op, anywhere.
    assert_eq!(allowed(d.door.check(&t, TENANT, APPLY)), None);
    assert_eq!(allowed(d.door.check(&t, HOME, APP_OP)), None);
    assert_eq!(allowed(d.door.check(&t, TENANT, APP_OP)), None);
    assert!(!d.door.presence_required(&t, HOME, APP_OP));

    // Every decision names the person.
    let record = d.record();
    assert_eq!(record.len(), 6, "{record:?}");
    assert!(
        record.iter().all(|(who, _)| who == "user:p_ada"),
        "{record:?}"
    );
    assert_eq!(
        record.iter().filter(|(_, d)| d.is_none()).count(),
        3,
        "three acts allowed"
    );
    assert!(record
        .iter()
        .filter_map(|(_, d)| *d)
        .all(|d| d == RecordedDecision::OutsideDeploymentRealm));
}

/// Plan section 7, "The door's" row: `kind`, `client` and `sid` from the
/// token beside `sub`, on an allowed act and on a denial alike.
#[tokio::test]
async fn every_decision_carries_the_tokens_kind_client_and_sid() {
    let issuer = Issuer::start().await;
    let k = key(1);
    issuer.publish(&[&k]);
    let d = door(&issuer.url, IdentityKeyStore::memory()).await;
    d.keys.boot().await.expect("booted");
    let mut c = claims(Kind::Machine, None);
    c.client = "cli".into();
    c.sid = "s_ci_7".into();
    let t = sign(&c, &k);

    assert!(allowed(d.door.check(&t, HOME, APPLY)).is_some());
    assert!(allowed(d.door.check(&t, HOME, APP_OP)).is_none());

    let events = d.door.take_events();
    assert_eq!(events.len(), 2, "{events:?}");
    for e in &events {
        assert_eq!(e.subject.render(), "user:m_ci");
        let facts = e.subject.token().expect("token facts on the record");
        assert_eq!(
            (
                facts.kind.as_str(),
                facts.client.as_str(),
                facts.sid.as_str()
            ),
            ("machine", "cli", "s_ci_7")
        );
        // In the hashed body: the entry decodes back with them.
        let decoded =
            ordo_secevent::decode_entry(&ordo_secevent::encode_entry(e)).expect("decodes");
        assert_eq!(&decoded, e);
    }
}

#[tokio::test]
async fn a_stale_touch_reads_and_is_presence_required_for_a_write_and_any_tenant_op() {
    let issuer = Issuer::start().await;
    let k = key(1);
    issuer.publish(&[&k]);
    let d = door(&issuer.url, IdentityKeyStore::memory()).await;
    d.keys.boot().await.expect("booted");
    let stale = sign(&claims(Kind::Person, Some(NOW - 301)), &k);

    assert_eq!(
        allowed(d.door.check(&stale, HOME, COST)),
        Some("p_ada".into())
    );
    assert_eq!(allowed(d.door.check(&stale, HOME, APPLY)), None);
    assert!(d.door.presence_required(&stale, HOME, APPLY));
    assert_eq!(allowed(d.door.check(&stale, TENANT, MEMBERS)), None);
    assert!(d.door.presence_required(&stale, TENANT, MEMBERS));
    // An op outside the realm is a denial, never a touch ask.
    assert!(!d.door.presence_required(&stale, HOME, APP_OP));
    assert!(!d.door.presence_required(&stale, TENANT, APPLY));
    let decisions: Vec<_> = d.record().into_iter().filter_map(|(_, d)| d).collect();
    assert_eq!(
        decisions,
        vec![RecordedDecision::StepUpStale, RecordedDecision::StepUpStale]
    );

    // A token with no touch at all is the same as a stale one.
    let untouched = sign(&claims(Kind::Person, None), &k);
    assert!(d.door.presence_required(&untouched, HOME, APPLY));
}

#[tokio::test]
async fn a_machine_key_applies_at_home_and_reaches_no_tenant() {
    let issuer = Issuer::start().await;
    let k = key(1);
    issuer.publish(&[&k]);
    let d = door(&issuer.url, IdentityKeyStore::memory()).await;
    d.keys.boot().await.expect("booted");
    let m = sign(&claims(Kind::Machine, None), &k);

    assert_eq!(allowed(d.door.check(&m, HOME, APPLY)), Some("m_ci".into()));
    assert_eq!(allowed(d.door.check(&m, TENANT, MEMBERS)), None);
    assert!(!d.door.presence_required(&m, TENANT, MEMBERS));
    assert_eq!(allowed(d.door.check(&m, HOME, APP_OP)), None);

    // Break-glass is a person with a fresh touch.
    let b = sign(&claims(Kind::Breakglass, Some(NOW)), &k);
    assert_eq!(
        allowed(d.door.check(&b, HOME, APPLY)),
        Some("breakglass:root".into())
    );
}

#[tokio::test]
async fn expired_wrong_audience_and_wrong_issuer_are_each_denied_and_recorded() {
    let issuer = Issuer::start().await;
    let k = key(1);
    issuer.publish(&[&k]);
    let d = door(&issuer.url, IdentityKeyStore::memory()).await;
    d.keys.boot().await.expect("booted");

    let t = sign(&claims(Kind::Person, Some(NOW)), &k);
    d.at(NOW + 600);
    assert_eq!(allowed(d.door.check(&t, HOME, COST)), None);
    assert!(!d.door.presence_required(&t, HOME, APPLY));
    d.at(NOW);

    let mut c = claims(Kind::Person, Some(NOW));
    c.aud = deployment_token::audience("other");
    assert_eq!(allowed(d.door.check(&sign(&c, &k), HOME, COST)), None);
    let mut c = claims(Kind::Person, Some(NOW));
    c.iss = "https://id.elsewhere.example".into();
    assert_eq!(allowed(d.door.check(&sign(&c, &k), HOME, COST)), None);
    // Signed by a key the service never published.
    let forged = sign(&claims(Kind::Person, Some(NOW)), &key(9));
    assert_eq!(allowed(d.door.check(&forged, HOME, COST)), None);

    let record = d.record();
    assert!(
        record.iter().all(|(who, _)| who == "unverified"),
        "{record:?}"
    );
    // One collapsed window per subject and op, so the order is the set's.
    let mut decisions: Vec<_> = record.into_iter().filter_map(|(_, d)| d).collect();
    decisions.sort();
    assert_eq!(
        decisions,
        vec![
            RecordedDecision::DeploymentTokenRefused,
            RecordedDecision::DeploymentTokenWrongIssuer,
            RecordedDecision::DeploymentTokenWrongAudience,
            RecordedDecision::DeploymentTokenExpired,
        ]
    );
}

#[tokio::test]
async fn an_unknown_kid_fetches_once_and_then_waits_out_the_gap() {
    let issuer = Issuer::start().await;
    let old = key(1);
    let new = key(2);
    issuer.publish(&[&old]);
    let d = door(&issuer.url, IdentityKeyStore::memory()).await;
    d.keys.boot().await.expect("booted");
    assert_eq!(issuer.hits(), 1);

    // The service adds a key; a token under it arrives before any refresh.
    issuer.publish(&[&old, &new]);
    let t = sign(&claims(Kind::Person, Some(NOW)), &new);
    assert_eq!(allowed(d.door.check(&t, HOME, COST)), None, "not held yet");
    assert_eq!(d.door.token_gate(&t).await, ordo_api::TokenGate::Pass);
    assert_eq!(issuer.hits(), 2, "one fetch for the unknown kid");
    assert_eq!(allowed(d.door.check(&t, HOME, COST)), Some("p_ada".into()));

    // Forged kids inside the gap fetch nothing more.
    for seed in 10..20 {
        let f = sign(&claims(Kind::Person, Some(NOW)), &key(seed));
        d.door.token_gate(&f).await;
    }
    assert_eq!(issuer.hits(), 2, "at most one fetch per gap");
    d.at(NOW + UNKNOWN_KID_GAP_SECS);
    d.door
        .token_gate(&sign(&claims(Kind::Person, Some(NOW)), &key(30)))
        .await;
    assert_eq!(issuer.hits(), 3, "the gap passed");

    // A held kid fetches nothing.
    d.at(NOW + 2 * UNKNOWN_KID_GAP_SECS);
    d.door.token_gate(&t).await;
    assert_eq!(issuer.hits(), 3);
}

#[tokio::test]
async fn the_service_down_still_verifies_from_the_stored_keys() {
    let issuer = Issuer::start().await;
    let k = key(1);
    issuer.publish(&[&k]);
    let store = IdentityKeyStore::memory();
    let first = door(&issuer.url, store.clone()).await;
    first.keys.boot().await.expect("booted");
    let t = sign(&claims(Kind::Person, Some(NOW)), &k);

    // The service stops serving keys; this door keeps what it holds.
    issuer.down();
    assert!(first.keys.refresh().await.is_err());
    assert_eq!(
        allowed(first.door.check(&t, HOME, COST)),
        Some("p_ada".into())
    );

    // A restart while it is down reads the stored set.
    let second = door(&issuer.url, store.clone()).await;
    let e = second.keys.boot().await.expect_err("the service is down");
    assert!(e.retryable(), "{e}");
    assert_eq!(
        allowed(second.door.check(&t, HOME, COST)),
        Some("p_ada".into())
    );

    // Nothing listening at all, and nothing stored: no keys, every token
    // refused, and the refusal says why.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let empty = door(&closed, IdentityKeyStore::memory()).await;
    let e = empty.keys.boot().await.expect_err("nothing listens");
    assert!(
        matches!(
            e,
            ordo_host::identity_keys::IdentityKeysError::Unreachable { .. }
        ),
        "{e}"
    );
    assert!(e.to_string().contains("ORDO_IDENTITY_URL"), "{e}");
    assert_eq!(allowed(empty.door.check(&t, HOME, COST)), None);
    assert_eq!(
        empty
            .record()
            .into_iter()
            .filter_map(|(_, d)| d)
            .collect::<Vec<_>>(),
        vec![RecordedDecision::DeploymentTokenRefused]
    );
}

#[tokio::test]
async fn a_rotation_verifies_the_new_key_and_a_retired_key_verifies_nothing() {
    let issuer = Issuer::start().await;
    let old = key(1);
    let new = key(2);
    issuer.publish(&[&old]);
    let d = door(&issuer.url, IdentityKeyStore::memory()).await;
    d.keys.boot().await.expect("booted");
    let by_old = sign(&claims(Kind::Person, Some(NOW)), &old);
    let by_new = sign(&claims(Kind::Person, Some(NOW)), &new);

    // The new pair is published an hour before it signs.
    issuer.publish(&[&old, &new]);
    d.keys.refresh().await.expect("refreshed");
    assert!(allowed(d.door.check(&by_old, HOME, COST)).is_some());
    assert!(allowed(d.door.check(&by_new, HOME, COST)).is_some());

    // The old pair is retired.
    issuer.publish(&[&new]);
    d.keys.refresh().await.expect("refreshed");
    assert_eq!(allowed(d.door.check(&by_old, HOME, COST)), None);
    assert!(allowed(d.door.check(&by_new, HOME, COST)).is_some());
}

#[tokio::test]
async fn an_app_token_never_reaches_the_identity_service() {
    let issuer = Issuer::start().await;
    issuer.publish(&[&key(1)]);
    let d = door(&issuer.url, IdentityKeyStore::memory()).await;
    d.keys.boot().await.expect("booted");
    let before = issuer.hits();
    let app = ordo_identity::token::mint(
        &TokenKey::from_secret(SECRET),
        &ordo_identity::TokenSpec {
            principal: PrincipalId("usr_owner".into()),
            expires: ordo_identity::Timestamp(NOW + 600),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    );
    assert!(!deployment_token::is_deployment_shape(&app));
    assert_eq!(d.door.token_gate(&app).await, ordo_api::TokenGate::Pass);
    // The app owner's own op answers as it always has.
    assert_eq!(
        allowed(d.door.check(&app, HOME, APP_OP)),
        Some("usr_owner".into())
    );
    assert!(!d.door.presence_required(&app, HOME, APPLY));
    assert_eq!(issuer.hits(), before, "no fetch for an app token");
}

#[tokio::test]
async fn a_door_with_no_identity_service_refuses_every_deployment_token() {
    let d = IdentityAccess::new(
        AccessEngine::new(),
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW),
        Arc::new(Metrics::new()),
    )
    .with_console_home(HOME);
    let t = sign(&claims(Kind::Person, Some(NOW)), &key(1));
    assert!(matches!(d.check(&t, HOME, COST), AccessDecision::Deny));
    assert!(!d.presence_required(&t, HOME, APPLY));
    assert!(!d.token_known(&t), "no live stream either");
}

#[tokio::test]
async fn the_auth_routes_refuse_a_deployment_token() {
    use ordo_host::auth::AuthState;
    use ordo_identity::directory::{Directory, Provisioning};

    let recorder = Arc::new(ordo_secevent::Recorder::default());
    let state = AuthState::new(
        ordo_identity::ProviderRegistry::new(),
        Directory::with_store(
            Provisioning::InviteOnly,
            Arc::new(ordo_identity::MemoryDirectoryStore::new()),
        ),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(ordo_identity::MemorySessionRevocations::new()))
    .with_recorder(recorder.clone());
    let app = ordo_host::auth::router(Some(Arc::new(state)));
    let t = sign(&claims(Kind::Person, Some(NOW)), &key(1));
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/logout")
                .header("authorization", format!("Bearer {t}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let reasons: Vec<String> = recorder
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::AuthenticationFailed { reason } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(reasons, vec!["deployment-token-at-auth-route".to_string()]);
}

/// `$identitykeys` on Postgres: a door that fetched once and restarts with
/// the service down reads the set back from the control database.
#[tokio::test]
async fn the_stored_keys_survive_a_restart_on_postgres() {
    let db = "ordo_identity_tokens";
    let f = ordo_conformance::Fixture::load().expect("fixture");
    f.reset(&ordo_conformance::Pg::named(db)).expect("database");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.into(),
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    let store = IdentityKeyStore::pg(pool.clone());
    store.ensure_table().await.expect("table");
    pool.rows(r#"DELETE FROM "$identitykeys""#).await.unwrap();

    let issuer = Issuer::start().await;
    let k = key(4);
    issuer.publish(&[&k]);
    let first = door(&issuer.url, store.clone()).await;
    first.keys.boot().await.expect("booted");
    issuer.down();

    let second = door(&issuer.url, IdentityKeyStore::pg(pool.clone())).await;
    assert!(second.keys.boot().await.is_err(), "the service is down");
    assert_eq!(second.keys.held_keys(), 1);
    let t = sign(&claims(Kind::Person, Some(NOW)), &k);
    assert_eq!(
        allowed(second.door.check(&t, HOME, COST)),
        Some("p_ada".into())
    );
}

/// `GET /security/events` is on the open tenant list, so a stale person gets
/// the realm's own answer there, `presence_required`, as at the op door. A
/// machine key stays the one `access_denied`.
#[tokio::test]
async fn the_security_events_route_gives_the_realm_answer() {
    let db = "ordo_identity_events_route";
    let f = Arc::new(ordo_conformance::Fixture::load().expect("fixture"));
    let pg = ordo_conformance::Pg::named(db);
    f.reset(&pg).expect("database");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.into(),
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    let issuer = Issuer::start().await;
    let k = key(1);
    issuer.publish(&[&k]);
    let d = door(&issuer.url, IdentityKeyStore::memory()).await;
    d.keys.boot().await.expect("booted");
    let deployment = ordo_host::deploy::Deployment::pinned(
        "events-route-test",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    let state = ordo_host::serve::PlatformState::new(
        Arc::new(d.door),
        deployment,
        pool,
        f.keys.clone(),
        HOME.to_string(),
        Arc::new(Metrics::new()),
    );
    let get = |token: String| {
        let app = ordo_host::serve::platform_router(state.clone());
        async move {
            let res = app
                .oneshot(
                    Request::builder()
                        .uri(format!("/security/events?space={TENANT}"))
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = res.status();
            let body = axum::body::to_bytes(res.into_body(), 1 << 20)
                .await
                .unwrap();
            let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let code = v["error"]["code"].as_str().or(v["error"].as_str());
            (status, code.unwrap_or("").to_string())
        }
    };
    let stale = sign(&claims(Kind::Person, Some(NOW - 301)), &k);
    assert_eq!(
        get(stale).await,
        (StatusCode::FORBIDDEN, "presence_required".to_string())
    );
    let machine = sign(&claims(Kind::Machine, None), &k);
    assert_eq!(
        get(machine).await,
        (StatusCode::FORBIDDEN, "access_denied".to_string())
    );
    let fresh = sign(&claims(Kind::Person, Some(NOW - 10)), &k);
    // Past the door: this state has no security stream, so the route says so.
    assert_ne!(get(fresh).await.0, StatusCode::FORBIDDEN);
}

/// With `ORDO_IDENTITY_URL` set the console calls the door as the person,
/// so a console key session (G14) minted before the setting was set
/// reaches nothing, not even a read in the home space, and the record
/// says why (`plan-identity-service.md` 6).
#[tokio::test]
async fn a_console_key_session_reaches_nothing_while_the_door_verifies_deployment_tokens() {
    let issuer = Issuer::start().await;
    let k = key(1);
    issuer.publish(&[&k]);
    let d = door(&issuer.url, IdentityKeyStore::memory()).await;
    let session = ordo_identity::SessionClaim::starting_at(ordo_identity::Timestamp(NOW)).with_via(
        format!("{}ck-{:032x}", ordo_host::console_keys::VIA_PREFIX, 7),
    );
    let spec = ordo_identity::TokenSpec {
        principal: PrincipalId("usr_owner".into()),
        expires: ordo_identity::Timestamp(NOW + 900),
        client: None,
        user_presence_at: Some(ordo_identity::Timestamp(NOW)),
        capability: None,
        delegation: None,
        session: Some(session),
    };
    let t = ordo_identity::token::mint(&TokenKey::from_secret(SECRET), &spec);
    assert_eq!(allowed(d.door.check(&t, HOME, COST)), None);
    assert_eq!(allowed(d.door.check(&t, HOME, APPLY)), None);
    let decisions: Vec<_> = d.record().into_iter().filter_map(|(_, d)| d).collect();
    assert!(!decisions.is_empty());
    assert!(
        decisions
            .iter()
            .all(|d| *d == RecordedDecision::ConsoleKeyClosed),
        "{decisions:?}"
    );
}
