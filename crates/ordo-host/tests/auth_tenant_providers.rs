//! **Tenant-owned sign-in providers** (`plan-tenant-providers.md`, L1;
//! owner rulings 2026-10-09). Through the real door for the ops, the real
//! sign-in routes, a real TLS issuer (`oidc_support`) and real Postgres.
//!
//! What it proves:
//!
//! - a tenant's owner adds an OIDC provider, and a newcomer signs in through
//!   it and lands in that tenant only;
//! - the client secret is sealed at rest and never answered;
//! - one tenant's connection cannot sign anybody in to another tenant;
//! - retiring a connection stops its sign-ins at once;
//! - with the setting off, the ops and `begin` refuse naming it;
//! - only an owner with presence may add one;
//! - the last owner who signs in without the tenant's providers cannot be
//!   demoted, and a provider cannot be added without one;
//! - an address resolving to the cloud metadata endpoint is refused at
//!   connect;
//! - a tenant connection's two-step claim counts only inside its tenant.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

mod oauth2_support;
mod oidc_support;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oauth2_support::{redeem, send};
use oidc_support::{param, FakeIdp, CLIENT_SECRET, PUBLIC, RETURN};
use ordo_api::{
    router, AccessChecker, AccessDecision, ApiState, MemoryBus, MemoryReplayCache, PageKey,
};
use ordo_conformance::{Fixture, Pg};
use ordo_egress::{DenyList, TlsRoots};
use ordo_host::admission::{Admission, Ceiling, PolicyChange, PolicyStore};
use ordo_host::auth::{AuthState, Enrolment};
use ordo_host::credential_ops::{CredentialOps, ManagedStore};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::member_ops::MemberOps;
use ordo_host::oidc::EgressFactory;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::principal_directory::PgDirectoryStore;
use ordo_host::runtime::SwappableAccess;
use ordo_host::tenant_providers::{self as tp, TenantConnections};
use ordo_host::tickets::{PgTicketStore, Tickets};
use ordo_host::FixtureExecutor;
use ordo_identity::principal::{Principal, PrincipalId, PrincipalKind, Timestamp as IStamp};
use ordo_identity::provider::Argon2idHasher;
use ordo_identity::{
    AccessEngine, AccessProfile, AuthenticatorId, DirectoryStore, MemoryCeremonyStore,
    MemoryDirectoryStore, MemorySessionRevocations, ProviderRegistry, Role, SpaceId, Timestamp,
    TokenKey, TokenSpec, UpstreamAcceptance,
};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const SECRET: [u8; 32] = *b"tenant-providers-test-secret-32b";
const CA: &[u8] = include_bytes!("fixture/ca.der");
const OWNER: &str = "mem_o";
const MANAGER: &str = "mem_1";
const OTHER: &str = "spc_other_tenant";
const HOME: &str = "spc_home_deployment";
const NAME: &str = "acme-sso";

fn now() -> u64 {
    ordo_host::runtime::now_secs()
}

struct H {
    api: ApiState,
    app: axum::Router,
    pool: ordo_exec::Pool,
    space: String,
    idp: FakeIdp,
    directory: Arc<dyn DirectoryStore>,
    admission: Arc<Admission>,
    /// Set by [`harness_with_credentials`].
    credentials: Option<Arc<CredentialOps>>,
    tc: Arc<TenantConnections>,
}

async fn harness(db: &str, enabled: bool) -> H {
    build(db, enabled, false).await
}

/// The harness with the directory and credentials in Postgres and
/// `std.space.resetCredential` wired, as a deployment has them.
async fn harness_with_credentials(db: &str) -> H {
    build(db, true, true).await
}

async fn build(db: &str, enabled: bool, with_credentials: bool) -> H {
    build_with(
        db,
        enabled,
        with_credentials,
        ordo_host::oidc::OidcDefaults::default(),
    )
    .await
}

async fn build_with(
    db: &str,
    enabled: bool,
    with_credentials: bool,
    oidc: ordo_host::oidc::OidcDefaults,
) -> H {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 3,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    let grants = Arc::new(PgGrantRegistry::new(pool.clone()));
    grants.ensure_table().await.expect("platform table");
    ordo_host::admission::ensure_table(&pool)
        .await
        .expect("admission");
    pool.rows("DROP TABLE IF EXISTS \"$tenantconnection\"")
        .await
        .expect("clean");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let engine = ordo_host::deployment_engine_pooled(&f, &pool)
        .await
        .expect("engine");
    let door = ordo_host::IdentityAccess::sharing(
        Arc::new(engine),
        TokenKey::from_secret(&SECRET),
        Box::new(now),
        metrics.clone(),
    );
    let access = Arc::new(SwappableAccess::new(door));
    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "tenant-providers-test",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        platform_ops::specs(),
    )
    .expect("the fixture compiles no `std.space.*` name");

    let cfg = || ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 3,
        ..ordo_exec::PoolConfig::from_env()
    };
    if with_credentials {
        ordo_host::principal_directory::ensure_table(&pool)
            .await
            .unwrap();
        ordo_host::credentials::ensure_table(&pool).await.unwrap();
        ordo_host::sessions::ensure_table(&pool).await.unwrap();
        ordo_host::tickets::ensure_table(&pool).await.unwrap();
        ordo_host::credential_ops::ensure_table(&pool)
            .await
            .unwrap();
        for t in [
            "$authenticator",
            "$principal",
            "$credential",
            "$ticket",
            "$managed",
            "$temporarypassword",
            "$principalepoch",
            "$sessionrevocation",
        ] {
            pool.rows(&format!("DELETE FROM \"{t}\""))
                .await
                .expect("clean");
        }
    }
    let members = Arc::new(MemberOps {
        control: pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
        directory: None,
        stream: Some(access.recorder.clone()),
        refresh: None,
        now: Arc::new(now),
        managed: with_credentials.then(|| ManagedStore::new(pool.clone())),
    });
    let admission = Arc::new(Admission {
        members: members.clone(),
        store: PolicyStore::new(pool.clone()),
        tickets: None,
        ceiling: Ceiling::everything(),
        home: Some(HOME.into()),
    });
    // The founding owner signs in with a passkey: the originating admin.
    let directory: Arc<dyn DirectoryStore> = if with_credentials {
        Arc::new(PgDirectoryStore::open(cfg()).expect("directory"))
    } else {
        Arc::new(MemoryDirectoryStore::new())
    };
    link(&*directory, OWNER, "passkey:owner-key");
    let ceremonies = Arc::new(MemoryCeremonyStore::new());
    let factory = Arc::new(EgressFactory::new(
        TlsRoots::custom(&[CA.to_vec()]).expect("anchor"),
        DenyList::categories_only()
            .permitting_loopback_for_tests()
            .for_tenant_providers(),
    ));
    let tc = Arc::new(
        TenantConnections::new(
            enabled,
            members.clone(),
            directory.clone(),
            Arc::new(ordo_crypto::TestProvider::new()),
            ceremonies.clone(),
            factory,
            PUBLIC,
            ordo_host::federation::parse_return_urls(RETURN, false).expect("returns"),
            HOME,
        )
        .with_admission(admission.clone())
        .with_oidc_defaults(oidc),
    );
    let live = access.clone();
    let mut ops = PlatformOps::new(access.current().engine.clone(), grants, Arc::new(now))
        .with_engine_source(Arc::new(move || live.current().engine.clone()))
        .with_members(members.clone())
        .with_admission(admission.clone());
    let credentials = if with_credentials {
        let revocations = Arc::new(
            ordo_host::sessions::PgSessionRevocations::open(pool.clone())
                .await
                .expect("revocations"),
        );
        let ops_c = Arc::new(CredentialOps {
            members: members.clone(),
            directory: directory.clone(),
            credentials: Arc::new(
                ordo_host::credentials::PgCredentialStore::open(cfg()).expect("credentials"),
            ),
            hasher: Arc::new(Argon2idHasher::weak_for_tests()),
            tickets: Arc::new(Tickets::new(Arc::new(
                PgTicketStore::open(cfg()).expect("tickets"),
            ))),
            managed: ManagedStore::new(pool.clone()),
            revocations,
            session_max_secs: 12 * 3600,
            allow_temporary: false,
            control: pool.clone(),
            after_redeem: Default::default(),
        });
        ops = ops.with_credentials(ops_c.clone());
        Some(ops_c)
    } else {
        None
    };
    let _ = ops.tenant_connections.set(tc.clone());
    let api = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: pg.clone(),
                page_size: 50,
            }),
            Arc::new(ops),
        )),
        access: access.clone(),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"tenant-providers-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };

    let passkey = Arc::new(ordo_identity::PasskeyProvider::with_stores(
        ordo_identity::RelyingParty::new("example.com", &["https://app.example.com"])
            .expect("a relying party"),
        Arc::new(ordo_identity::MemoryPasskeyStore::new()),
        ceremonies.clone(),
    ));
    let mut registry = ProviderRegistry::new();
    registry
        .register("passkey", Box::new(passkey.clone()))
        .expect("registered");
    let mut state = AuthState::new(
        registry,
        ordo_identity::directory::Directory::with_store(
            ordo_identity::directory::Provisioning::AutoOnboard,
            directory.clone(),
        ),
        TokenKey::from_secret(&SECRET),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()))
    .with_passkey(passkey)
    .with_ceremonies(ceremonies);
    state.now = Box::new(now);
    state.wire_enrolment(Enrolment {
        tickets: None,
        admission: Some(admission.clone()),
        credentials: None,
    });
    state.wire_tenant_connections(tc.clone());
    H {
        api,
        app: ordo_host::auth::router(Some(Arc::new(state))),
        pool,
        space: ordo_conformance::fixture_space().0,
        idp: FakeIdp::start().await,
        directory,
        admission,
        credentials,
        tc,
    }
}

fn link(directory: &dyn DirectoryStore, principal: &str, authenticator: &str) {
    directory
        .insert_link(
            &AuthenticatorId(authenticator.into()),
            &Principal {
                id: PrincipalId(principal.into()),
                kind: PrincipalKind::Human,
                display: principal.into(),
                created: IStamp(1),
            },
            IStamp(1),
        )
        .expect("linked");
}

fn token(principal: &str, presence: bool) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(&SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(9_000_000_000),
            client: None,
            user_presence_at: presence.then(|| IStamp(now())),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

/// A platform op through the door, as `who`, with or without presence.
async fn op(h: &H, name: &str, who: &str, presence: bool, mut body: Value) -> (StatusCode, Value) {
    body["$space"] = json!(h.space);
    let read = name == tp::LIST;
    let resp = router(h.api.clone())
        .oneshot(
            Request::builder()
                .method(if read { "GET" } else { "POST" })
                .uri(format!("/op/{name}"))
                .header("x-ordo-space", &h.space)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token(who, presence)))
                .body(if read {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

/// The owner adds an OIDC connection to the stand-in issuer.
async fn add(h: &H, issuer: &str) -> (StatusCode, Value) {
    op(
        h,
        tp::ADD,
        OWNER,
        true,
        json!({"name": NAME, "kind": "oidc", "issuer": issuer,
               "clientId": oidc_support::CLIENT_ID, "clientSecret": CLIENT_SECRET,
               "role": "editor"}),
    )
    .await
}

async fn added(h: &H) -> Value {
    let (status, out) = add(h, &h.idp.issuer()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    out["rows"][0].clone()
}

/// The page's `begin` at a tenant: `(status, body)`.
async fn begin(h: &H, tenant: &str, extra: Value) -> (StatusCode, Value) {
    let mut body = json!({"method": NAME, "tenant": tenant, "return_to": RETURN});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        body[k] = v;
    }
    let (status, _, out) = send(&h.app, "POST", "/auth/begin", Some(body), None).await;
    (status, out)
}

async fn callback(h: &H, tenant: &str, state: &str, code: &str) -> (StatusCode, Option<String>) {
    let (status, location, _) = send(
        &h.app,
        "GET",
        &format!(
            "/auth/callback/t/{tenant}/{NAME}?state={}&code={}",
            ordo_identity::federation::percent(state),
            ordo_identity::federation::percent(code)
        ),
        None,
        None,
    )
    .await;
    (status, location)
}

fn nelly(extra: Value) -> Value {
    let mut c = json!({"sub": "acme-0001", "name": "Nelly Acme",
        "email": "nelly@acme.example", "email_verified": true, "iat": now()});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        c[k] = v;
    }
    c
}

/// Begin at the tenant, sign in at the issuer, come back: `(handoff, binder)`.
async fn through_the_issuer(
    h: &H,
    code_word: &str,
    begin_extra: Value,
    claims: Value,
) -> (String, String) {
    let (status, begun) = begin(h, &h.space, begin_extra).await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let url = begun["redirect"].as_str().expect("a redirect").to_string();
    assert_eq!(
        param(&url, "redirect_uri").as_deref(),
        Some(format!("{PUBLIC}/auth/callback/t/{}/{NAME}", h.space).as_str())
    );
    h.idp.authorize(&url, code_word, claims);
    let state = param(&url, "state").expect("a state");
    let (status, location) = callback(h, &h.space, &state, code_word).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let handoff = param(&location.expect("a location"), "handoff").expect("a handoff");
    (handoff, begun["binder"].as_str().unwrap().to_string())
}

async fn member_role(h: &H, principal: &str) -> Option<String> {
    h.pool
        .rows(&format!(
            "SELECT \"role\" FROM \"members\" WHERE \"$space\" = '{}' AND \"principal\" = '{principal}'",
            h.space
        ))
        .await
        .expect("members")
        .first()
        .and_then(|r| r.first().cloned().flatten())
}

fn principal_of(done: &Value) -> PrincipalId {
    ordo_identity::verify(
        &TokenKey::from_secret(&SECRET),
        done["token"].as_str().expect("a token"),
        Timestamp(now()),
    )
    .expect("a token this deployment minted")
    .principal
}

#[tokio::test(flavor = "multi_thread")]
async fn a_newcomer_signs_in_through_the_tenants_provider_and_lands_in_that_tenant_only() {
    let h = harness("ordo_tp_newcomer", true).await;
    let row = added(&h).await;
    assert_eq!(
        row["callback"],
        format!("{PUBLIC}/auth/callback/t/{}/{NAME}", h.space)
    );
    assert_eq!(row["secret"], "set");
    let key = row["key"].as_str().expect("a key").to_string();
    assert!(ordo_identity::upstream::tenant_key_of(
        &key,
        &SpaceId(h.space.clone())
    ));

    let (handoff, binder) = through_the_issuer(&h, "code-1", json!({}), nelly(json!({}))).await;
    let (status, done) = redeem(&h.app, NAME, &handoff, &binder, json!({"tenant": h.space})).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["via"], format!("oidc:{key}"));
    assert_eq!(done["admission"]["space"], h.space.as_str());
    assert_eq!(done["admission"]["outcome"], "admitted");
    assert!(done.get("given").is_none(), "no tenant of their own");
    let who = principal_of(&done);
    assert_eq!(member_role(&h, &who.0).await.as_deref(), Some("editor"));
    assert_eq!(
        h.directory
            .principal_for(&AuthenticatorId(format!("oidc:{key}:acme-0001")))
            .unwrap(),
        Some(who.clone())
    );
    // Not a member anywhere else.
    let elsewhere = h
        .pool
        .rows(&format!(
            "SELECT count(*) FROM \"members\" WHERE \"principal\" = '{}' AND \"$space\" <> '{}'",
            who.0, h.space
        ))
        .await
        .unwrap();
    assert_eq!(elsewhere[0][0].as_deref(), Some("0"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_client_secret_is_sealed_at_rest_and_never_answered() {
    let h = harness("ordo_tp_secret", true).await;
    let row = added(&h).await;
    assert!(!row.to_string().contains(CLIENT_SECRET));
    let (status, listed) = op(&h, tp::LIST, OWNER, true, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["rows"][0]["name"], NAME);
    assert_eq!(listed["rows"][0]["secret"], "set");
    assert!(!listed.to_string().contains(CLIENT_SECRET), "{listed}");
    let stored = h
        .pool
        .rows("SELECT \"secret\", \"settings\" FROM \"$tenantconnection\"")
        .await
        .unwrap();
    let all = format!("{stored:?}");
    assert!(!all.contains(CLIENT_SECRET));
    // A rotated secret is sealed too, and still never answered.
    let (status, out) = op(
        &h,
        tp::UPDATE,
        OWNER,
        true,
        json!({"name": NAME, "clientSecret": "rotated-secret-never-shown"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert!(!out.to_string().contains("rotated-secret-never-shown"));
    // The issuer cannot change under the same connection.
    let (_, out) = op(
        &h,
        tp::UPDATE,
        OWNER,
        true,
        json!({"name": NAME, "issuer": "https://elsewhere.example"}),
    )
    .await;
    assert_eq!(code(&out), "tenant_refused", "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn one_tenants_connection_cannot_sign_in_to_another() {
    let h = harness("ordo_tp_cross", true).await;
    added(&h).await;
    // Another tenant has no connection by that name.
    let (status, _) = begin(&h, OTHER, json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A ceremony begun at this tenant does not return through another's
    // callback.
    let (status, begun) = begin(&h, &h.space, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let url = begun["redirect"].as_str().unwrap().to_string();
    h.idp.authorize(&url, "code-x", nelly(json!({})));
    let state = param(&url, "state").unwrap();
    let (status, _) = callback(&h, OTHER, &state, "code-x").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Its handoff is refused for another tenant, both ways of naming one.
    let (handoff, binder) = through_the_issuer(&h, "code-2", json!({}), nelly(json!({}))).await;
    let (status, _) = redeem(&h.app, NAME, &handoff, &binder, json!({"tenant": OTHER})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = redeem(
        &h.app,
        NAME,
        &handoff,
        &binder,
        json!({"tenant": h.space, "space": OTHER}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Without `tenant` the name is no deployment method either.
    let (status, _) = redeem(&h.app, NAME, &handoff, &binder, json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // At its own tenant it still completes.
    let (status, done) = redeem(&h.app, NAME, &handoff, &binder, json!({"tenant": h.space})).await;
    assert_eq!(status, StatusCode::OK, "{done}");
}

#[tokio::test(flavor = "multi_thread")]
async fn retiring_a_connection_stops_its_sign_ins_at_once() {
    let h = harness("ordo_tp_retire", true).await;
    added(&h).await;
    let (status, begun) = begin(&h, &h.space, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let url = begun["redirect"].as_str().unwrap().to_string();
    h.idp.authorize(&url, "code-r", nelly(json!({})));
    let (status, out) = op(&h, tp::RETIRE, OWNER, true, json!({"name": NAME})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["rows"][0]["state"], "retired");
    // A new begin, and the return of one begun before, are both refused.
    let (status, _) = begin(&h, &h.space, json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = callback(&h, &h.space, &param(&url, "state").unwrap(), "code-r").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(h.idp.state.lock().unwrap().token_requests.is_empty());
    // The name may be added again, under a new key.
    let again = added(&h).await;
    assert_ne!(again["key"], out["rows"][0]["key"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn with_the_setting_off_the_ops_and_begin_name_it() {
    let h = harness("ordo_tp_off", false).await;
    let (status, out) = add(&h, &h.idp.issuer()).await;
    assert_ne!(status, StatusCode::OK);
    assert_eq!(code(&out), "tenant_refused", "{out}");
    assert!(out["error"]["message"]
        .as_str()
        .unwrap()
        .contains("ORDO_AUTH_TENANT_PROVIDERS=on"));
    let (status, out) = begin(&h, &h.space, json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&out), "tenant_providers_off");
    assert_eq!(out["error"]["setting"], "ORDO_AUTH_TENANT_PROVIDERS");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_an_owner_with_presence_may_add_a_provider() {
    let h = harness("ordo_tp_owner", true).await;
    let body = json!({"name": NAME, "kind": "oidc", "issuer": h.idp.issuer(),
        "clientId": "c", "clientSecret": "s"});
    let (status, out) = op(&h, tp::ADD, MANAGER, true, body.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{out}");
    let (status, out) = op(&h, tp::ADD, OWNER, false, body).await;
    assert_ne!(status, StatusCode::OK, "{out}");
    let (_, listed) = op(&h, tp::LIST, OWNER, true, json!({})).await;
    assert_eq!(listed["rows"], json!([]), "{listed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_last_owner_who_signs_in_without_the_tenants_provider_is_kept() {
    let h = harness("ordo_tp_admin", true).await;
    // Without such an owner, no provider is added.
    h.directory
        .remove_link(&AuthenticatorId("passkey:owner-key".into()))
        .unwrap();
    let (_, out) = add(&h, &h.idp.issuer()).await;
    assert_eq!(code(&out), "tenant_refused", "{out}");
    link(&*h.directory, OWNER, "passkey:owner-key");
    let row = added(&h).await;
    let key = row["key"].as_str().unwrap();
    // A second owner whose only sign-in is the tenant's own provider.
    let (status, out) = op(
        &h,
        ordo_host::member_ops::SET_ROLE,
        OWNER,
        true,
        json!({"member": MANAGER, "role": "owner"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    link(&*h.directory, MANAGER, &format!("oidc:{key}:manager-sub"));
    // Demoting the founding owner would leave only them: refused, by name.
    let (_, out) = op(
        &h,
        ordo_host::member_ops::SET_ROLE,
        OWNER,
        true,
        json!({"member": OWNER, "role": "viewer"}),
    )
    .await;
    assert_eq!(code(&out), "tenant_refused", "{out}");
    assert!(out["error"]["message"]
        .as_str()
        .unwrap()
        .contains("without its own providers"));
    let (_, out) = op(
        &h,
        ordo_host::member_ops::REMOVE,
        OWNER,
        true,
        json!({"member": OWNER}),
    )
    .await;
    assert_eq!(code(&out), "tenant_refused", "{out}");
    // Once the second owner holds a passkey too, it goes through.
    link(&*h.directory, MANAGER, "passkey:manager-key");
    let (status, out) = op(
        &h,
        ordo_host::member_ops::SET_ROLE,
        OWNER,
        true,
        json!({"member": OWNER, "role": "viewer"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_provider_on_the_metadata_address_is_refused_at_connect() {
    let h = harness("ordo_tp_metadata", true).await;
    // No address is checked when it is added: a name can change what it
    // resolves to. The check is at connect, after resolution.
    let (status, out) = add(&h, "https://169.254.169.254").await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out) = begin(&h, &h.space, json!({})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{out}");
    assert_eq!(code(&out), "identity_provider_unavailable");
    let message = out["error"]["message"].as_str().unwrap();
    assert!(message.contains("cloud metadata"), "{message}");
    assert!(!message.contains("ORDO_EGRESS_PRIVATE_HOSTS"), "{message}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_connections_two_step_claim_counts_only_inside_that_tenant() {
    let h = harness("ordo_tp_presence", true).await;
    let row = added(&h).await;
    let key = row["key"].as_str().unwrap().to_string();
    // The tenant lists its own connection, by key. Another tenant cannot.
    h.admission
        .set_policy(
            &h.space,
            &PolicyChange {
                presence_from: Some(vec![key.clone()]),
                ..Default::default()
            },
            OWNER,
            now(),
        )
        .await
        .expect("its own key");
    let refused = h
        .admission
        .set_policy(
            OTHER,
            &PolicyChange {
                presence_from: Some(vec![key.clone()]),
                ..Default::default()
            },
            OWNER,
            now(),
        )
        .await;
    assert!(refused.is_err(), "another tenant's key is refused");

    let claims = nelly(json!({"amr": ["pwd", "mfa"], "auth_time": now()}));
    let (handoff, binder) =
        through_the_issuer(&h, "code-p", json!({"presence": true}), claims).await;
    let (status, done) = redeem(&h.app, NAME, &handoff, &binder, json!({"tenant": h.space})).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let token = done["token"].as_str().unwrap();
    let up = ordo_identity::verify(&TokenKey::from_secret(&SECRET), token, Timestamp(now()))
        .unwrap()
        .session
        .and_then(|s| s.upstream_presence)
        .expect("a two-step claim");
    assert_eq!(up.via(), key);

    // The door: the person owns this tenant, another and the deployment's
    // own space, and every one of them asks for presence on `export`.
    const EXPORT: &str = "std.space.export";
    let who = principal_of(&done);
    let acceptance = Arc::new(
        UpstreamAcceptance::new()
            .with_deployment_space(SpaceId(HOME.into()))
            .with_deployment_ops(ordo_host::presence::DEPLOYMENT_ACTS),
    );
    for s in [h.space.as_str(), OTHER, HOME] {
        acceptance.set_tenant(&SpaceId(s.into()), [key.clone()].into());
    }
    let mut engine = AccessEngine::new();
    for s in [h.space.as_str(), OTHER, HOME] {
        let s = SpaceId(s.into());
        engine.set_profile(&s, AccessProfile::new().allow(Role::Owner, EXPORT));
        engine.set_membership(&s, &who, Role::Owner);
    }
    engine.require_step_up(EXPORT);
    engine.accept_upstream(acceptance);
    let door = ordo_host::IdentityAccess::new(
        engine,
        TokenKey::from_secret(&SECRET),
        Box::new(now),
        Arc::new(ordo_host::metrics::Metrics::new()),
    );
    let allowed = |space: &str| {
        matches!(
            door.check(token, space, EXPORT),
            AccessDecision::Allow { .. }
        )
    };
    assert!(allowed(&h.space), "inside its own tenant");
    assert!(!allowed(OTHER), "not in another tenant");
    assert!(!allowed(HOME), "not in the deployment's own space");
}

/// The body a browser posts for a passkey ceremony.
fn passkey_body(req: &ordo_identity::CompleteRequest) -> Value {
    use ordo_identity::webauthn::b64url_encode;
    match req {
        ordo_identity::CompleteRequest::PasskeyRegistration {
            challenge,
            client_data_json,
            attestation_object,
        } => json!({"method": "passkey", "kind": "passkey_registration",
            "challenge": challenge.0,
            "client_data_json": b64url_encode(client_data_json),
            "attestation_object": b64url_encode(attestation_object)}),
        ordo_identity::CompleteRequest::PasskeyAssertion {
            challenge,
            credential_id,
            client_data_json,
            authenticator_data,
            signature,
        } => json!({"method": "passkey", "kind": "passkey_assertion",
            "challenge": challenge.0,
            "credential_id": b64url_encode(credential_id),
            "client_data_json": b64url_encode(client_data_json),
            "authenticator_data": b64url_encode(authenticator_data),
            "signature": b64url_encode(signature)}),
        _ => panic!("not a passkey ceremony"),
    }
}

/// A person who signed up through a tenant's provider adds a passkey by
/// claim, as a Discord-only account can ("a login method is a login
/// method"). The passkey names the same person and reaches nothing more.
#[tokio::test(flavor = "multi_thread")]
async fn a_person_from_a_tenants_provider_adds_a_passkey_by_claim() {
    let h = harness("ordo_tp_claim", true).await;
    added(&h).await;
    let (handoff, binder) = through_the_issuer(&h, "code-1", json!({}), nelly(json!({}))).await;
    let (status, done) = redeem(&h.app, NAME, &handoff, &binder, json!({"tenant": h.space})).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let token = done["token"].as_str().unwrap().to_string();
    let who = principal_of(&done);

    // A fresh ceremony through the tenant's provider opens the window.
    let (handoff, binder) = through_the_issuer(&h, "code-2", json!({}), nelly(json!({}))).await;
    let first = json!({"method": NAME, "tenant": h.space, "kind": "handoff",
                       "handoff": handoff, "binder": binder});
    let (status, _, opened) = send(
        &h.app,
        "POST",
        "/auth/claim/begin",
        Some(first),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let window = opened["claim"].as_str().unwrap().to_string();
    let (status, _, options) = send(
        &h.app,
        "POST",
        "/auth/claim/passkey",
        Some(json!({"claim": window})),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let device = ordo_identity::software_authenticator::TestAuthenticator::es256();
    let mut body = passkey_body(&device.register(options["challenge"].as_str().unwrap()));
    body["claim"] = json!(window);
    let (status, _, linked) = send(
        &h.app,
        "POST",
        "/auth/claim/complete",
        Some(body),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{linked}");
    assert_eq!(linked["linked"], device.authenticator_id().0);

    // The passkey signs in as the same person, and admits them nowhere new.
    let (status, _, options) = send(
        &h.app,
        "POST",
        "/auth/begin",
        Some(json!({"method": "passkey"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let mut body = passkey_body(&device.assert(options["challenge"].as_str().unwrap()));
    body["space"] = json!(OTHER);
    let (status, _, by_passkey) = send(&h.app, "POST", "/auth/complete", Some(body), None).await;
    assert_eq!(status, StatusCode::OK, "{by_passkey}");
    assert_eq!(principal_of(&by_passkey), who);
    assert_eq!(by_passkey["admission"]["outcome"], "refused");
    let memberships = h
        .pool
        .rows(&format!(
            "SELECT \"$space\" FROM \"members\" WHERE \"principal\" = '{}'",
            who.0
        ))
        .await
        .unwrap();
    assert_eq!(memberships.len(), 1);
    assert_eq!(memberships[0][0].as_deref(), Some(h.space.as_str()));
}

/// Two owners with passkeys are demoted at the same moment (the door here
/// is not rebuilt, so the founding owner may still ask), leaving only an
/// owner who signs in through the tenant's provider. Each alone would pass;
/// the tenant's lock takes them one after the other, so one is refused.
#[tokio::test(flavor = "multi_thread")]
async fn two_concurrent_changes_cannot_both_leave_no_originating_admin() {
    let h = harness("ordo_tp_race", true).await;
    let key = added(&h).await["key"].as_str().unwrap().to_string();
    const SECOND: &str = "mem_2";
    for (member, sign_in) in [
        (MANAGER, "passkey:manager-key".to_string()),
        (SECOND, format!("oidc:{key}:second-sub")),
    ] {
        let (status, out) = op(
            &h,
            ordo_host::member_ops::SET_ROLE,
            OWNER,
            true,
            json!({"member": member, "role": "owner"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{out}");
        link(&*h.directory, member, &sign_in);
    }
    for round in 0..3 {
        let demote = |member: &'static str| {
            op(
                &h,
                ordo_host::member_ops::SET_ROLE,
                OWNER,
                true,
                json!({"member": member, "role": "viewer"}),
            )
        };
        let ((a, out_a), (b, out_b)) = tokio::join!(demote(OWNER), demote(MANAGER));
        let passed = [a, b].iter().filter(|s| **s == StatusCode::OK).count();
        assert_eq!(passed, 1, "round {round}: {out_a} {out_b}");
        let refused = if a == StatusCode::OK { &out_b } else { &out_a };
        assert_eq!(code(refused), "tenant_refused", "{refused}");
        // Put both back for the next round.
        for member in [OWNER, MANAGER] {
            let (status, out) = op(
                &h,
                ordo_host::member_ops::SET_ROLE,
                OWNER,
                true,
                json!({"member": member, "role": "owner"}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{out}");
        }
    }
}

// ── a credential reset keeps the originating admin too ──────────────────

/// The owner creates a person who chooses a password, makes them an owner
/// and gives them a sign-in through the tenant's provider. Returns
/// `(principal, member, password identifier)`.
async fn password_owner(h: &H, key: &str, name: &str) -> (String, String, String) {
    let identifier = format!("{name}@example.test");
    let (status, out) = op(
        h,
        ordo_host::credential_ops::CREATE_PERSON,
        OWNER,
        true,
        json!({"display": name}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let row = &out["rows"][0];
    let (principal, member) = (
        row["principal"].as_str().unwrap().to_string(),
        row["member"].as_str().unwrap().to_string(),
    );
    h.credentials
        .as_ref()
        .unwrap()
        .redeem_password(
            row["ticket"].as_str().unwrap(),
            &identifier,
            "correct-horse",
        )
        .await
        .expect("the person chooses a password");
    let (status, out) = op(
        h,
        ordo_host::member_ops::SET_ROLE,
        OWNER,
        true,
        json!({"member": member, "role": "owner"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    link(&*h.directory, &principal, &format!("oidc:{key}:{name}-sub"));
    (principal, member, identifier)
}

async fn reset(h: &H, member: &str) -> (StatusCode, Value) {
    op(
        h,
        ordo_host::credential_ops::RESET_CREDENTIAL,
        OWNER,
        true,
        json!({"member": member}),
    )
    .await
}

/// The founding owner's passkey is swapped for a sign-in through the
/// tenant's provider, so only password owners sign in without it.
fn founder_uses_the_provider(h: &H, key: &str) {
    h.directory
        .remove_link(&AuthenticatorId("passkey:owner-key".into()))
        .unwrap();
    link(&*h.directory, OWNER, &format!("oidc:{key}:founder-sub"));
}

fn holds_password(h: &H, principal: &str, identifier: &str) -> bool {
    let held = h
        .directory
        .authenticators_of(&PrincipalId(principal.into()))
        .unwrap();
    held.iter().any(|a| a.0 == format!("password:{identifier}"))
        && h.credentials
            .as_ref()
            .unwrap()
            .credentials
            .hash_for(identifier)
            .unwrap()
            .is_some()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_credential_reset_that_would_leave_no_originating_admin_is_refused() {
    let h = harness_with_credentials("ordo_tp_reset_refused").await;
    let key = added(&h).await["key"].as_str().unwrap().to_string();
    let (principal, member, identifier) = password_owner(&h, &key, "ana").await;
    founder_uses_the_provider(&h, &key);
    // Ana's password is the only sign-in an owner has outside the provider.
    let (_, out) = reset(&h, &member).await;
    assert_eq!(code(&out), "tenant_refused", "{out}");
    assert!(out["error"]["message"]
        .as_str()
        .unwrap()
        .contains("without its own providers"));
    assert!(
        holds_password(&h, &principal, &identifier),
        "the refused reset changed nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_credential_reset_is_allowed_while_another_originating_admin_remains() {
    let h = harness_with_credentials("ordo_tp_reset_allowed").await;
    let key = added(&h).await["key"].as_str().unwrap().to_string();
    let (principal, member, identifier) = password_owner(&h, &key, "ana").await;
    // The founding owner keeps the passkey: another originating admin.
    let (status, out) = reset(&h, &member).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["rows"][0]["outcome"], "reset", "{out}");
    assert!(!holds_password(&h, &principal, &identifier));
}

/// Two owners each hold a password and no other way in outside the provider;
/// the founder signs in through the provider. Resetting both at once would
/// leave nobody, so the tenant's lock lets one through.
#[tokio::test(flavor = "multi_thread")]
async fn two_concurrent_credential_resets_cannot_both_leave_no_originating_admin() {
    let h = harness_with_credentials("ordo_tp_reset_race").await;
    let key = added(&h).await["key"].as_str().unwrap().to_string();
    let (_, ana, _) = password_owner(&h, &key, "ana").await;
    let (_, ben, _) = password_owner(&h, &key, "ben").await;
    founder_uses_the_provider(&h, &key);
    let ((a, out_a), (b, out_b)) = tokio::join!(reset(&h, &ana), reset(&h, &ben));
    let passed = [a, b].iter().filter(|s| **s == StatusCode::OK).count();
    assert_eq!(passed, 1, "{out_a} {out_b}");
    let refused = if a == StatusCode::OK { &out_b } else { &out_a };
    assert_eq!(code(refused), "tenant_refused", "{refused}");
}

/// `GET /auth/methods`, with no token: `(status, body text, body)`.
async fn methods(h: &H, query: &str) -> (StatusCode, String, Value) {
    let res = h
        .app
        .clone()
        .oneshot(
            Request::get(format!("/auth/methods{query}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    let text = String::from_utf8_lossy(&bytes).to_string();
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, text, body)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_methods_route_lists_a_tenants_active_providers_and_an_unknown_tenant_looks_like_none()
{
    let h = harness("ordo_tp_methods", true).await;
    let (_, _, bare) = methods(&h, "").await;
    assert_eq!(bare["connections"], json!([]));

    added(&h).await;
    let (status, text, body) = methods(&h, &format!("?tenant={}", h.space)).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(
        body["connections"],
        json!([{"name": NAME, "label": NAME, "kind": "oidc", "tenant": true}])
    );
    assert_eq!(body["passkey"], true);
    let issuer = h.idp.issuer();
    for hidden in [
        CLIENT_SECRET,
        oidc_support::CLIENT_ID,
        issuer.as_str(),
        "editor",
    ] {
        assert!(!text.contains(hidden), "`{hidden}` is in {text}");
    }
    // Without the tenant query the deployment's methods are all there is.
    let (_, _, plain) = methods(&h, "").await;
    assert_eq!(plain, bare);

    // A tenant with none, one that does not exist, the home space and a
    // malformed id all look the same.
    for tenant in [OTHER, "spc_nobody_here", HOME, "not%20a%20space"] {
        let (status, _, other) = methods(&h, &format!("?tenant={tenant}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(other, bare, "tenant `{tenant}`");
    }

    // A retired provider drops out at once.
    let (status, out) = op(&h, tp::RETIRE, OWNER, true, json!({"name": NAME})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, _, after) = methods(&h, &format!("?tenant={}", h.space)).await;
    assert_eq!(after, bare);
}

#[tokio::test(flavor = "multi_thread")]
async fn with_tenant_providers_off_a_tenant_query_adds_nothing() {
    let h = harness("ordo_tp_methods_off", false).await;
    let (_, _, bare) = methods(&h, "").await;
    let (status, _, body) = methods(&h, &format!("?tenant={}", h.space)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, bare);
}

/// A tenant's connection carries no algorithms of its own, so it takes the
/// deployment's `ORDO_AUTH_OIDC_ALGS`. Here that is ES256 alone, and the
/// issuer signs RS256: the sign-in fails at the callback (audit 2026-10-11,
/// finding 18).
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_connection_takes_the_deployments_algorithms() {
    let h = build_with(
        "ordo_tp_algs",
        true,
        false,
        ordo_host::oidc::OidcDefaults {
            algs: vec![ordo_identity::jose::Alg::ES256],
            ..Default::default()
        },
    )
    .await;
    added(&h).await;
    let (status, begun) = begin(&h, &h.space, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let url = begun["redirect"].as_str().expect("a redirect").to_string();
    h.idp.authorize(&url, "code-algs", nelly(json!({})));
    let state = param(&url, "state").expect("a state");
    let (_, location) = callback(&h, &h.space, &state, "code-algs").await;
    let handoff = location.as_deref().and_then(|l| param(l, "handoff"));
    assert!(handoff.is_none(), "an RS256 token is refused: {location:?}");
}

/// The lock wait is `ORDO_AUTH_TENANT_LOCK_WAIT_MS`, and a change that waits
/// it out is refused naming the setting (audit 2026-10-11, finding 19).
#[tokio::test(flavor = "multi_thread")]
async fn the_tenant_lock_wait_is_the_setting_and_its_refusal_names_it() {
    let h = harness("ordo_tp_lockwait", true).await;
    let short = TenantConnections::new(
        true,
        h.tc.members.clone(),
        h.tc.directory.clone(),
        h.tc.keys.clone(),
        h.tc.ceremonies.clone(),
        h.tc.transports.clone(),
        PUBLIC,
        h.tc.returns.clone(),
        HOME,
    )
    .with_lock_wait_ms(100);
    // Warm `short` (its table check) while the lock is free, so the timed
    // call is the wait and not one-time setup.
    let got = short.lock(&h.space).await.expect("free");
    TenantConnections::release(vec![got]).await;
    let held =
        h.tc.lock(&h.space)
            .await
            .expect("the first change holds it");
    let started = std::time::Instant::now();
    let e = short
        .lock(&h.space)
        .await
        .err()
        .expect("the second waits it out");
    // The setting, not the default: a wait of the default refuses no
    // earlier than its 10 s. The bound used to be 2 s, which measured the
    // database's round trips too, and a runner whose Postgres stalled for
    // 8 s failed it (CI for f198df49). How close the refusal comes to the
    // setting is `the_tenant_lock_refusal_comes_at_the_deadline_warm_and_cold`.
    let took = started.elapsed();
    assert!(took >= ms(100), "{took:?}");
    assert!(took < ms(tp::DEFAULT_LOCK_WAIT_MS), "{took:?}");
    assert!(
        e.to_string().contains("ORDO_AUTH_TENANT_LOCK_WAIT_MS=100"),
        "{e}"
    );
    TenantConnections::release(vec![held]).await;
}

// ── the tenant lock's wait is a deadline (reported fix, CI run 39181144502) ──
//
// The wait used to be counted in tries, `wait / 25 ms` of them, and each try
// paid a checkout, a query and a rollback before its pause. On a slow
// database the refusal for a 100 ms wait came after more than two seconds.
// The wait is now wall-clock time from the call, and every ask after the
// first is cut at the deadline. A regression shows as a refusal that comes
// late, or as a waiter that sits on a connection between asks.

const SECOND: std::time::Duration = std::time::Duration::from_secs(1);

fn ms(n: u64) -> std::time::Duration {
    std::time::Duration::from_millis(n)
}

/// A replica of `h`'s deployment: its own pool onto the same database, of
/// `size` connections, so its first ask dials (a cold pool).
async fn replica(
    h: &H,
    size: usize,
    acquire: std::time::Duration,
    wait_ms: u64,
) -> TenantConnections {
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        size,
        acquire_timeout: acquire,
        ..h.pool.config()
    })
    .await
    .expect("a replica's pool");
    let members = Arc::new(MemberOps {
        control: pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(pool)),
        directory: None,
        stream: None,
        refresh: None,
        now: Arc::new(now),
        managed: None,
    });
    TenantConnections::new(
        true,
        members,
        h.tc.directory.clone(),
        h.tc.keys.clone(),
        h.tc.ceremonies.clone(),
        h.tc.transports.clone(),
        PUBLIC,
        h.tc.returns.clone(),
        HOME,
    )
    .with_lock_wait_ms(wait_ms)
}

/// Another handle on `h`'s own pool, warm, waiting `wait_ms`.
fn warm(h: &H, wait_ms: u64) -> TenantConnections {
    TenantConnections::new(
        true,
        h.tc.members.clone(),
        h.tc.directory.clone(),
        h.tc.keys.clone(),
        h.tc.ceremonies.clone(),
        h.tc.transports.clone(),
        PUBLIC,
        h.tc.returns.clone(),
        HOME,
    )
    .with_lock_wait_ms(wait_ms)
}

/// The refusal for a 300 ms wait comes after 300 ms and well before 1.5 s,
/// on a warm pool and on a cold one, and names the setting and its value.
#[tokio::test(flavor = "multi_thread")]
async fn the_tenant_lock_refusal_comes_at_the_deadline_warm_and_cold() {
    let h = harness("ordo_tp_lock_deadline", true).await;
    let held =
        h.tc.lock(&h.space)
            .await
            .expect("the first change holds it");
    let cold = replica(&h, 2, 2 * SECOND, 300).await;
    for (name, waiter) in [("warm", warm(&h, 300)), ("cold", cold)] {
        let started = std::time::Instant::now();
        let e = waiter.lock(&h.space).await.err().expect("it waits it out");
        let took = started.elapsed();
        assert!(took >= ms(300), "{name}: {took:?}");
        assert!(took < ms(1500), "{name}: {took:?}");
        assert!(
            e.to_string().contains("ORDO_AUTH_TENANT_LOCK_WAIT_MS=300"),
            "{name}: {e}"
        );
    }
    TenantConnections::release(vec![held]).await;
}

/// A wait of 0 asks once: refused at once while the lock is held, and
/// taken at once when it is free.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_lock_wait_of_zero_asks_once() {
    let h = harness("ordo_tp_lock_zero", true).await;
    let zero = warm(&h, 0);
    // Its table check first, so the timed call is the ask alone.
    let got = zero.lock(&h.space).await.expect("free");
    TenantConnections::release(vec![got]).await;
    let held = h.tc.lock(&h.space).await.expect("held");
    let started = std::time::Instant::now();
    let e = zero.lock(&h.space).await.err().expect("refused");
    // Refused without waiting a wait: well short of the default's 10 s. A
    // tighter bound measures the database's round trip, not the wait.
    assert!(
        started.elapsed() < ms(tp::DEFAULT_LOCK_WAIT_MS),
        "{:?}",
        started.elapsed()
    );
    assert!(
        e.to_string().contains("ORDO_AUTH_TENANT_LOCK_WAIT_MS=0"),
        "{e}"
    );
    TenantConnections::release(vec![held]).await;

    let got = warm(&h, 0).lock(&h.space).await.expect("free, so taken");
    TenantConnections::release(vec![got]).await;
}

/// A holder that lets go part way through the wait lets the waiter in,
/// before the deadline.
///
/// Judged by order and outcome, not by a stopwatch. `lock` refuses once
/// its deadline passes, so an `Ok` is the waiter getting in before it. The
/// waiter is still waiting when the holder lets go, and gets in only after.
/// A wall-clock bound here failed on a loaded CI runner (8.2 s against a
/// 4 s bound, 2026-10-23): the time went to the runner, not to the lock.
/// The deadline's own timing is guarded by
/// `the_tenant_lock_refusal_comes_at_the_deadline_warm_and_cold`.
#[tokio::test(flavor = "multi_thread")]
async fn a_released_tenant_lock_lets_the_waiter_in_before_the_deadline() {
    let h = harness("ordo_tp_lock_handover", true).await;
    let held = h.tc.lock(&h.space).await.expect("held");
    // A wait no runner's stall can use up, so only the release ends it.
    let waiter = Arc::new(warm(&h, 60_000));
    let space = h.space.clone();
    let w = waiter.clone();
    let waiting = tokio::spawn(async move {
        let got = w.lock(&space).await;
        (got, std::time::Instant::now())
    });
    tokio::time::sleep(ms(300)).await;
    assert!(!waiting.is_finished(), "the waiter waits while it is held");
    let let_go = std::time::Instant::now();
    TenantConnections::release(vec![held]).await;
    let (got, at) = waiting.await.expect("no panic");
    let got = got.expect("the waiter gets it once the holder lets go, before its deadline");
    assert!(
        at >= let_go,
        "the waiter got in only after the holder let go"
    );
    TenantConnections::release(vec![got]).await;
}

/// Two replicas, each with its own pool onto the tenant's database, take
/// the lock in turns: the lock lives in Postgres, not in a process.
#[tokio::test(flavor = "multi_thread")]
async fn two_replicas_take_the_tenant_lock_in_turns() {
    let h = harness("ordo_tp_lock_replicas", true).await;
    let a = Arc::new(replica(&h, 2, 2 * SECOND, 5_000).await);
    let b = Arc::new(replica(&h, 2, 2 * SECOND, 5_000).await);
    for (first, second) in [(a.clone(), b.clone()), (b.clone(), a.clone())] {
        let held = first
            .lock(&h.space)
            .await
            .expect("the first replica holds it");
        let refused = warm(&h, 100).lock(&h.space).await;
        assert!(refused.is_err(), "held across replicas");
        let space = h.space.clone();
        let waiter = tokio::spawn(async move { second.lock(&space).await });
        tokio::time::sleep(ms(200)).await;
        assert!(!waiter.is_finished(), "the second replica waits");
        TenantConnections::release(vec![held]).await;
        let got = tokio::time::timeout(4 * SECOND, waiter)
            .await
            .expect("in time")
            .expect("no panic")
            .expect("the second replica takes it");
        TenantConnections::release(vec![got]).await;
    }
}

/// A waiting change holds no connection between asks: on a replica whose
/// pool has one connection, the test takes that connection while the change
/// waits, and nothing in this database is parked waiting on an advisory
/// lock.
///
/// Judged by outcome, not by a stopwatch. The change waits 60 s, so no
/// stall of the runner's database can use its wait up while the test
/// probes; only the release ends it. The test's own checkout waits up to
/// 20 s: a change that sat on the one connection for its wait would make
/// that checkout fail, and one that lets go between asks makes it wait one
/// ask at most. A 3 s wait and a 1 s bound on the checkout failed when the
/// runner's Postgres stalled for 8 s (CI for f198df49).
#[tokio::test(flavor = "multi_thread")]
async fn a_waiting_tenant_change_holds_no_connection_between_asks() {
    let h = harness("ordo_tp_lock_noconn", true).await;
    let one = replica(&h, 1, 20 * SECOND, 60_000).await;
    let pool = one.members.control.clone();
    // Warm the replica (its table check) before the lock is taken.
    let got = one.lock(&h.space).await.expect("free");
    TenantConnections::release(vec![got]).await;

    let held = h.tc.lock(&h.space).await.expect("held");
    let space = h.space.clone();
    let waiter = tokio::spawn(async move { one.lock(&space).await });
    for _ in 0..5 {
        tokio::time::sleep(ms(60)).await;
        let tx = pool
            .begin()
            .await
            .expect("the one connection is free between asks");
        tx.rollback().await.expect("returned");
        // This database only: the command log and the security record
        // take blocking advisory locks, and the other tests' databases on
        // the same cluster may be holding them.
        let parked = h
            .pool
            .rows(
                "SELECT count(*) FROM pg_stat_activity \
                 WHERE wait_event_type = 'Lock' AND wait_event = 'advisory' \
                   AND datname = current_database()",
            )
            .await
            .expect("activity");
        assert_eq!(
            parked[0][0].as_deref(),
            Some("0"),
            "nothing waits inside Postgres"
        );
    }
    assert!(!waiter.is_finished(), "still waiting");
    TenantConnections::release(vec![held]).await;
    // Bounded by the change's own 60 s deadline: past it, it is refused.
    let got = waiter
        .await
        .expect("no panic")
        .expect("taken once released");
    TenantConnections::release(vec![got]).await;
}

/// A slow ask is cut at the deadline. The replica's pool has two
/// connections and a 10 s checkout wait. Once the change has made its first
/// ask, the test takes both, so every later ask waits on the pool. A count
/// of tries waited out the pool's own 10 s; the deadline refuses at the
/// setting.
///
/// The refusal is timed from the later of the deadline and the moment the
/// pool ran dry. From then on the change only waits on the pool and its
/// timer, with no database round trip, so a slow database cannot move it.
/// The first ask is never cut (a wait of 0 still asks once), so if it is
/// still running when the test takes the connections, the test's second
/// checkout waits for it. Timing from before those checkouts failed with
/// 8.5 s when the runner's Postgres stalled (CI for f198df49).
#[tokio::test(flavor = "multi_thread")]
async fn a_slow_tenant_lock_ask_is_cut_at_the_deadline() {
    let h = harness("ordo_tp_lock_slow", true).await;
    let slow = replica(&h, 2, 10 * SECOND, 500).await;
    let pool = slow.members.control.clone();
    let got = slow.lock(&h.space).await.expect("free");
    TenantConnections::release(vec![got]).await;

    let held = h.tc.lock(&h.space).await.expect("held");
    let space = h.space.clone();
    let started = std::time::Instant::now();
    let waiter = tokio::spawn(async move { slow.lock(&space).await });
    tokio::time::sleep(ms(100)).await;
    let a = pool.begin().await.expect("first connection");
    let b = pool.begin().await.expect("second connection");
    let dry = std::time::Instant::now();
    let e = tokio::time::timeout(5 * SECOND, waiter)
        .await
        .expect("refused well inside the pool's own wait")
        .expect("no panic")
        .err()
        .expect("refused");
    let refused = std::time::Instant::now();
    assert!(refused - started >= ms(500), "{:?}", refused - started);
    let due = dry.max(started + ms(500));
    assert!(
        refused.saturating_duration_since(due) < SECOND,
        "{:?} after the deadline or the pool running dry",
        refused.saturating_duration_since(due)
    );
    assert!(
        e.to_string().contains("ORDO_AUTH_TENANT_LOCK_WAIT_MS=500"),
        "{e}"
    );
    let _ = a.rollback().await;
    let _ = b.rollback().await;
    TenantConnections::release(vec![held]).await;
}
