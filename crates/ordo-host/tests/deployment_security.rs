//! Console step 9 (`plan-console.md` 4.8 and 7), through the real door
//! against real Postgres:
//!
//! - **G8**, `std.deployment.securityEvents`: the deployment's own security
//!   stream, read by the deployment owner in the deployment's own space,
//!   paged, filtered by kind, tenant and time, and carrying no secret.
//!
//! The entries are written by the real producers where one is cheap to
//! drive (`setQuota` records `quota-set`), and appended directly otherwise.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_commandlog::{SpaceId as CSpace, Timestamp};
use ordo_conformance::{Fixture, Pg};
use ordo_exec::secevent::PgSecurityStore;
use ordo_host::grants::PgGrantRegistry;
use ordo_host::pause::PausePolicy;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::security_record::{self, SecurityRecordOps, EVENTS};
use ordo_host::FixtureExecutor;
use ordo_host::{quotas, tenants};
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use ordo_secevent::{correlation, EventKind, RecordedDecision, Recorder, Scope, StreamEntry};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: [u8; 32] = *b"deployment-security-test-secret!";
/// The conformance seed's owner of the tenant `conformance`.
const TENANT_OWNER: &str = "mem_o";
const DEPLOYMENT_OWNER: &str = "dep_owner";
const MANAGER: &str = "dep_manager";
const HOME: &str = "home";

struct Harness {
    state: ApiState,
    pool: ordo_exec::Pool,
    stream: Arc<Recorder>,
    store: PgSecurityStore,
    space: String,
}

async fn harness(db: &str) -> Harness {
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
    pool.ensure_platform_tables()
        .await
        .expect("platform tables");
    tenants::ensure_table(&pool).await.expect("registry");
    quotas::ensure_table(&pool).await.expect("quotas");
    let space = ordo_conformance::fixture_space().0;
    tenants::reserve(&pool, &space, "Conformance", TENANT_OWNER, NOW, None)
        .await
        .expect("reserved");
    assert!(tenants::activate(&pool, &space, NOW).await.expect("active"));

    let grants = Arc::new(PgGrantRegistry::new(pool.clone()));
    grants.ensure_table().await.expect("platform table");
    let mut rows = pool.rows(ordo_host::MEMBERS_SQL).await.expect("members");
    for (who, role) in [(DEPLOYMENT_OWNER, "owner"), (MANAGER, "manager")] {
        rows.push(vec![
            Some(HOME.into()),
            Some(who.into()),
            Some(role.into()),
            Some("active".into()),
            Some(who.into()),
        ]);
    }
    let engine = Arc::new(
        ordo_host::engine_from_members_with(
            &f,
            &rows,
            &ordo_host::presence::PresencePolicy::standard(),
            &PausePolicy::deployment_only(),
        )
        .expect("engine"),
    );
    let access = Arc::new(ordo_host::IdentityAccess::sharing(
        engine.clone(),
        TokenKey::from_secret(&SECRET),
        Box::new(ordo_host::runtime::now_secs),
        Arc::new(ordo_host::metrics::Metrics::new()),
    ));
    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "deployment-security-test",
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
    .expect("the fixture compiles no platform-op name");
    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());
    let stream = Arc::new(Recorder::default());
    let ops = Arc::new(
        PlatformOps::new(engine, grants, Arc::new(|| NOW))
            .with_quotas(Arc::new(quotas::QuotaOps {
                control: pool.clone(),
                home: HOME.into(),
                enforced: false,
                stream: Some(stream.clone()),
                now: Arc::new(|| NOW),
            }))
            .with_security_record(Arc::new(SecurityRecordOps {
                control: pool.clone(),
                home: HOME.into(),
            })),
    );
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces,
                pg: pg.clone(),
                page_size: 50,
            }),
            ops,
        )),
        access,
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"deployment-security-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    let store = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    Harness {
        state,
        pool,
        stream,
        store,
        space,
    }
}

fn token(principal: &str, touched: bool) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(&SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(9_000_000_000),
            client: None,
            user_presence_at: touched.then(|| IStamp(ordo_host::runtime::now_secs())),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

async fn send(h: &Harness, req: Request<Body>) -> (StatusCode, Value) {
    let resp = router(h.state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// `std.deployment.securityEvents` as `who`, bound to `space`.
async fn events(h: &Harness, space: &str, who: &str, query: &str) -> (StatusCode, Value) {
    send(
        h,
        Request::get(format!("/op/{EVENTS}{query}"))
            .header("x-ordo-space", space)
            .header("authorization", format!("Bearer {}", token(who, false)))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

fn rows(body: &Value) -> Vec<Value> {
    body["rows"].as_array().cloned().unwrap_or_default()
}

fn indices(body: &Value) -> Vec<u64> {
    rows(body)
        .iter()
        .map(|r| r["index"].as_u64().unwrap())
        .collect()
}

/// One deployment-scope entry about `tenant`, at `at`.
fn entry(kind: EventKind, tenant: Option<&str>, at: u64) -> StreamEntry {
    let mut e = StreamEntry::single(
        Scope::Deployment,
        ordo_secevent::Subject::member("dep_owner"),
        "std.deployment.apply",
        kind,
        Timestamp(at),
        correlation::mint(),
    );
    e.about = tenant.map(|t| CSpace(t.to_string()));
    e
}

fn deploy(act: &str) -> EventKind {
    EventKind::Deploy {
        act: act.into(),
        detail: format!("version=v1 by={DEPLOYMENT_OWNER}"),
    }
}

/// Seven entries: indices 0 to 6, at NOW + index.
async fn seed(h: &Harness) {
    let all = vec![
        entry(deploy("applied"), None, NOW),
        entry(
            EventKind::denied(RecordedDecision::NoMembership),
            Some("spc_a"),
            NOW + 1,
        ),
        entry(deploy("activated"), None, NOW + 2),
        entry(
            EventKind::denied(RecordedDecision::OpNotInRole),
            Some("spc_b"),
            NOW + 3,
        ),
        entry(
            EventKind::Enrolment {
                act: "tenant-paused".into(),
                detail: "tenant=spc_a by=dep_owner".into(),
            },
            Some("spc_a"),
            NOW + 4,
        ),
        entry(EventKind::Authenticated, None, NOW + 5),
        entry(deploy("applied"), None, NOW + 6),
    ];
    // A tenant's own stream is not the deployment's, and never answers here.
    let theirs = StreamEntry::single(
        Scope::Space(CSpace("spc_a".into())),
        ordo_secevent::Subject::member("mem_x"),
        "std.space.links",
        EventKind::denied(RecordedDecision::NoMembership),
        Timestamp(NOW),
        correlation::mint(),
    );
    h.store.try_append_all(&all).await.expect("appended");
    h.store.try_append(&theirs).await.expect("appended");
}

// ── G8 ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn only_the_deployment_owner_reads_the_record_and_only_in_the_home_space() {
    let h = harness("ordo_dsec_owner").await;
    seed(&h).await;
    let (status, body) = events(&h, HOME, DEPLOYMENT_OWNER, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(rows(&body).len(), 7, "{body}");

    // A manager of the deployment's own space is not its owner.
    let (status, _) = events(&h, HOME, MANAGER, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A tenant's owner is no one in the home space.
    let (status, _) = events(&h, HOME, TENANT_OWNER, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // In their own tenant the op does not answer: the record is the
    // deployment's.
    let (status, body) = events(&h, &h.space.clone(), TENANT_OWNER, "").await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
        "{status} {body}"
    );
    assert!(!body.to_string().contains("tenant-paused"), "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_record_pages_newest_first_or_oldest_first_from_an_index() {
    let h = harness("ordo_dsec_paging").await;
    seed(&h).await;
    let (_, body) = events(&h, HOME, DEPLOYMENT_OWNER, "?limit=3").await;
    assert_eq!(indices(&body), vec![6, 5, 4]);
    let (_, body) = events(&h, HOME, DEPLOYMENT_OWNER, "?limit=3&before=4").await;
    assert_eq!(indices(&body), vec![3, 2, 1]);
    let (_, body) = events(&h, HOME, DEPLOYMENT_OWNER, "?limit=3&before=1").await;
    assert_eq!(indices(&body), vec![0]);
    let (_, body) = events(&h, HOME, DEPLOYMENT_OWNER, "?from=5").await;
    assert_eq!(indices(&body), vec![5, 6]);

    // Both directions at once, and no rows a page, are refused.
    let (status, body) = events(&h, HOME, DEPLOYMENT_OWNER, "?from=1&before=4").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = events(&h, HOME, DEPLOYMENT_OWNER, "?limit=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_record_filters_by_kind_tenant_and_time() {
    let h = harness("ordo_dsec_filter").await;
    seed(&h).await;
    let (_, body) = events(&h, HOME, DEPLOYMENT_OWNER, "?kind=deploy").await;
    assert_eq!(indices(&body), vec![6, 2, 0]);
    assert_eq!(rows(&body)[1]["act"], "activated");

    let (_, body) = events(&h, HOME, DEPLOYMENT_OWNER, "?tenant=spc_a").await;
    assert_eq!(indices(&body), vec![4, 1]);
    assert!(rows(&body).iter().all(|r| r["tenant"] == "spc_a"));
    let denied = &rows(&body)[1];
    assert_eq!(denied["kind"], "denied");
    assert_eq!(denied["decisions"], json!(["NoMembership"]));

    let (_, body) = events(&h, HOME, DEPLOYMENT_OWNER, "?kind=denied&tenant=spc_b").await;
    assert_eq!(indices(&body), vec![3]);

    let q = format!("?since={}&until={}", NOW + 2, NOW + 4);
    let (_, body) = events(&h, HOME, DEPLOYMENT_OWNER, &q).await;
    assert_eq!(indices(&body), vec![4, 3, 2]);

    // A kind the stream does not spell is refused naming the ones it does.
    let (status, body) = events(&h, HOME, DEPLOYMENT_OWNER, "?kind=bogus").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("console-key"), "{body}");
    for k in security_record::KINDS {
        let (status, body) = events(&h, HOME, DEPLOYMENT_OWNER, &format!("?kind={k}")).await;
        assert_eq!(status, StatusCode::OK, "{k}: {body}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_row_is_the_recorded_act_and_never_more_than_the_record() {
    let h = harness("ordo_dsec_secrets").await;
    // A real producer: setQuota records quota-set in the deployment stream.
    let (status, body) = send(
        &h,
        Request::post(format!("/op/{}", quotas::SET_QUOTA))
            .header("x-ordo-space", HOME)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!("Bearer {}", token(DEPLOYMENT_OWNER, true)),
            )
            .body(Body::from(
                json!({"space": h.space, "quantity": "requests", "limit": 9,
                       "grace": 1, "periodSecs": 3600})
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let drained = h.stream.drain_all(Timestamp(NOW + 10_000_000_000));
    assert!(!drained.is_empty());
    h.store.try_append_all(&drained).await.expect("appended");

    let (status, body) = events(&h, HOME, DEPLOYMENT_OWNER, "?kind=enrolment").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = rows(&body);
    let set = rows
        .iter()
        .find(|r| r["act"] == "quota-set")
        .unwrap_or_else(|| panic!("no quota-set in {body}"));
    assert_eq!(set["tenant"], h.space);
    assert!(set["subject"].as_str().unwrap().contains(DEPLOYMENT_OWNER));
    assert!(set["detail"].as_str().unwrap().contains("limit=9"));
    // Not made on a deployment token, so no session facts.
    assert_eq!(set["session"], serde_json::Value::Null);

    // Exactly the documented fields: no chain hash, body or correlation,
    // and no token, key or setting value has a field to arrive in.
    let mut keys: Vec<&str> = set
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "act",
            "count",
            "decisions",
            "detail",
            "firstAt",
            "index",
            "kind",
            "lastAt",
            "op",
            "session",
            "subject",
            "subjectKind",
            "tenant"
        ]
    );
    let text = body.to_string();
    let bearer = token(DEPLOYMENT_OWNER, true);
    assert!(!text.contains(&bearer[..24]), "a token leaked: {text}");
    assert!(!text.contains("deployment-security-test"), "{text}");
    let _ = &h.pool;
}
