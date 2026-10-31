//! Console step 5 (`plan-console.md` 4.4 and 7), through the real door
//! against real Postgres:
//!
//! - **G4**, `std.deployment.tenants`: every tenant with its id, name,
//!   status, when made, the database it is on, its member count, its last
//!   usage period and the caller's role there. Never its members. The
//!   deployment owner's, in the deployment's own space only.
//! - **G12, the tenants part**: each write the console makes on a tenant
//!   through the door leaves a record. Create, pause, resume and reap.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::pause::{PauseOps, PausePolicy};
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::FixtureExecutor;
use ordo_host::{fleet, pause, reap_op, tenant_ops, tenants};
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use ordo_secevent::Recorder;
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: [u8; 32] = *b"deployment-tenants-test-secret32";
/// The conformance seed's owner of the tenant `conformance`.
const TENANT_OWNER: &str = "mem_o";
/// The deployment owner, an owner of the deployment's own space.
const DEPLOYMENT_OWNER: &str = "dep_owner";
const HOME: &str = "home";

struct Harness {
    state: ApiState,
    pool: ordo_exec::Pool,
    stream: Arc<Recorder>,
    /// The conformance tenant's id.
    space: String,
}

/// The conformance tenant, registered and active, its members, and the
/// deployment owner in the home space. With `owner_there`, the deployment
/// owner also owns the conformance tenant.
async fn harness(db: &str, owner_there: bool) -> Harness {
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
    fleet::ensure_tables(&pool).await.expect("fleet tables");
    let space = ordo_conformance::fixture_space().0;
    tenants::reserve(&pool, &space, "Conformance", TENANT_OWNER, NOW, None)
        .await
        .expect("reserved");
    assert!(tenants::activate(&pool, &space, NOW).await.expect("active"));

    let grants = Arc::new(PgGrantRegistry::new(pool.clone()));
    grants.ensure_table().await.expect("platform table");
    let mut rows = pool.rows(ordo_host::MEMBERS_SQL).await.expect("members");
    let row = |space: &str, role: &str| {
        vec![
            Some(space.into()),
            Some(DEPLOYMENT_OWNER.into()),
            Some(role.into()),
            Some("active".into()),
            Some(DEPLOYMENT_OWNER.into()),
        ]
    };
    rows.push(row(HOME, "owner"));
    if owner_there {
        rows.push(row(&space, "owner"));
    }
    let engine = Arc::new(
        ordo_host::engine_from_members_with(
            &f,
            &rows,
            &ordo_host::presence::PresencePolicy::everything_required(),
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
        "deployment-tenants-test",
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
    pause::reload(&spaces, &pool).await.expect("register");

    let stream = Arc::new(Recorder::default());
    let credentials = ordo_exec::route::WithOwnCredential::wrapping(
        Arc::new(ordo_exec::route::Installed::new().as_configured("local")),
        ordo_exec::PoolConfig::from_env().password.clone(),
    );
    let template = ordo_exec::PoolConfig {
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    };
    let ops = Arc::new(
        PlatformOps::new(engine, grants, Arc::new(|| NOW))
            .with_tenants(tenant_ops::TenantOps::with_routing(
                pool.clone(),
                f.keys.clone(),
                HOME,
                tenant_ops::Routing {
                    serving: deployment.clone(),
                    credentials: credentials.clone(),
                    template: template.clone(),
                    hosts: ordo_host::provision::HostAllowlist::of(
                        &ordo_exec::PoolConfig::from_env().host,
                    ),
                    default_cluster: tenant_ops::TenantCluster::from_env(
                        &ordo_exec::PoolConfig::from_env(),
                    ),
                    stream: Some(stream.clone()),
                },
            ))
            .with_reap(Arc::new(reap_op::ReapOps {
                pool: pool.clone(),
                keys: f.keys.clone(),
                home: HOME.to_string(),
                credentials,
                template,
                stream: Some(stream.clone()),
                now: Arc::new(|| NOW),
            }))
            .with_pause(Arc::new(PauseOps {
                control: pool.clone(),
                spaces: spaces.clone(),
                home: HOME.into(),
                stream: Some(stream.clone()),
                now: Arc::new(|| NOW),
            }))
            .with_tenant_list(Arc::new(fleet::TenantListOps {
                control: pool.clone(),
                home: HOME.into(),
                spaces: Some(spaces.clone()),
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
        page_key: PageKey::from_secret(b"deployment-tenants-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        state,
        pool,
        stream,
        space,
    }
}

/// A person's token, present, so a write's step-up is met.
fn token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(&SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(9_000_000_000),
            client: None,
            user_presence_at: Some(IStamp(ordo_host::runtime::now_secs())),
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

/// `std.deployment.tenants`, as `who`, bound to `space`.
async fn list(h: &Harness, space: &str, who: &str, query: &str) -> (StatusCode, Value) {
    send(
        h,
        Request::get(format!("/op/{}{query}", fleet::TENANTS))
            .header("x-ordo-space", space)
            .header("authorization", format!("Bearer {}", token(who)))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

/// A write op in the home space, as the deployment owner.
async fn command(h: &Harness, op: &str, body: Value, key: &str) -> (StatusCode, Value) {
    let mut body = body;
    body["$space"] = json!(HOME);
    send(
        h,
        Request::post(format!("/op/{op}"))
            .header("x-ordo-space", HOME)
            .header("content-type", "application/json")
            .header("idempotency-key", key)
            .header(
                "authorization",
                format!("Bearer {}", token(DEPLOYMENT_OWNER)),
            )
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

fn row<'a>(rows: &'a Value, id: &str) -> &'a Value {
    rows["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("no row for {id} in {rows}"))
}

fn records(h: &Harness) -> String {
    format!(
        "{:?}",
        h.stream
            .drain_all(ordo_commandlog::Timestamp(NOW + 10_000_000_000))
    )
}

// ── G4 ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn the_deployment_owner_lists_every_tenant_with_status_counts_and_usage() {
    let h = harness("ordo_dt_list", false).await;
    // A tenant still being built, one that failed, one reaped.
    tenants::reserve(&h.pool, "spc_building", "Building", "x", NOW + 1, None)
        .await
        .unwrap();
    tenants::reserve(&h.pool, "spc_broken", "Broken", "x", NOW + 2, None)
        .await
        .unwrap();
    tenants::fail(&h.pool, "spc_broken", "custody refused", NOW + 3)
        .await
        .unwrap();
    tenants::reserve(&h.pool, "spc_gone", "Gone", "x", NOW + 4, None)
        .await
        .unwrap();
    tenants::fail(&h.pool, "spc_gone", "stopped", NOW + 4)
        .await
        .unwrap();
    tenants::reaped(&h.pool, "spc_gone", NOW + 5).await.unwrap();
    // One rolled-up usage period for the conformance tenant.
    ordo_exec::usage::upsert_space(
        &h.pool,
        &ordo_exec::usage::SpaceRow {
            space: ordo_commandlog::SpaceId(h.space.clone()),
            period_start: 3_600,
            period_end: 7_200,
            chain_from: 0,
            chain_to: 9,
            writes: 9,
            rows_written: 12,
            bytes_sealed: 100,
            bytes_chain: 200,
            bytes_blob: 300,
            bytes_rows_estimated: 400,
            method_version: ordo_exec::usage::METHOD_VERSION.into(),
        },
    )
    .await
    .unwrap();

    let (status, body) = list(&h, HOME, DEPLOYMENT_OWNER, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "a reaped tenant is history: {body}");
    // Newest first.
    assert_eq!(rows[0]["id"], "spc_broken");

    let c = row(&body, &h.space);
    assert_eq!(c["name"], "Conformance");
    assert_eq!(c["status"], "active");
    assert_eq!(c["createdAt"], NOW);
    assert_eq!(c["target"], "default");
    assert_eq!(c["custody"], "platform");
    assert!(c["members"].as_u64().unwrap() >= 1, "{c}");
    assert_eq!(c["usage"]["writes"], 9);
    assert_eq!(c["usage"]["rowsWritten"], 12);
    assert_eq!(c["usage"]["periodEnd"], 7_200);
    assert!(c["quarantined"].is_null());
    assert!(c["yourRole"].is_null(), "not a member there: {c}");
    // Counts, never people: no principal is named anywhere.
    assert!(!body.to_string().contains(TENANT_OWNER), "{body}");

    let b = row(&body, "spc_building");
    assert_eq!(b["status"], "provisioning");
    assert_eq!(b["phase"], "reserved");
    assert!(b["members"].is_null());
    assert!(b["usage"].is_null());
    let f = row(&body, "spc_broken");
    assert_eq!(f["status"], "failed");
    assert_eq!(f["note"], "custody refused");

    let (_, all) = list(&h, HOME, DEPLOYMENT_OWNER, "?includeReaped=true").await;
    assert_eq!(row(&all, "spc_gone")["status"], "reaped");

    // A pause shows who paused and why.
    let (status, body) = command(
        &h,
        pause::PAUSE_TENANT,
        json!({"tenant": h.space, "note": "billing"}),
        "dt-pause",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = list(&h, HOME, DEPLOYMENT_OWNER, "").await;
    let c = row(&body, &h.space);
    assert_eq!(c["status"], "paused");
    assert_eq!(c["pausedBy"], "deployment");
    assert_eq!(c["pausedNote"], "billing");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_list_names_the_role_the_deployment_owner_holds_in_a_tenant() {
    let h = harness("ordo_dt_role", true).await;
    let (status, body) = list(&h, HOME, DEPLOYMENT_OWNER, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(row(&body, &h.space)["yourRole"], "owner");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_list_answers_only_the_deployment_owner_in_its_own_space() {
    let h = harness("ordo_dt_scope", true).await;
    // A tenant's owner, in their tenant: not theirs to read.
    let (status, body) = list(&h, &h.space.clone(), TENANT_OWNER, "").await;
    assert_ne!(status, StatusCode::OK, "{body}");
    // The deployment owner, bound to a tenant they own: the op is the
    // deployment's and answers only in its own space.
    let (status, body) = list(&h, &h.space.clone(), DEPLOYMENT_OWNER, "").await;
    assert_ne!(status, StatusCode::OK, "{body}");
    // Somebody with no role in the home space.
    let (status, body) = list(&h, HOME, TENANT_OWNER, "").await;
    assert_ne!(status, StatusCode::OK, "{body}");
}

// ── G12, the tenants part ────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn creating_pausing_resuming_and_reaping_a_tenant_each_leave_a_record() {
    let h = harness("ordo_dt_records", false).await;

    let (status, body) = command(
        &h,
        tenant_ops::CREATE,
        json!({"name": "Recorded", "owner": "usr_recorded"}),
        "dt-create",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["rows"][0]["space"].as_str().unwrap().to_string();
    let text = records(&h);
    assert!(text.contains("tenant-created"), "{text}");
    assert!(
        text.contains(&format!("tenant={id} owner=usr_recorded result=created")),
        "{text}"
    );
    assert!(text.contains(DEPLOYMENT_OWNER), "who did it: {text}");

    command(
        &h,
        pause::PAUSE_TENANT,
        json!({"tenant": id, "note": "maintenance"}),
        "dt-p",
    )
    .await;
    command(&h, pause::RESUME_TENANT, json!({"tenant": id}), "dt-r").await;
    let text = records(&h);
    assert!(text.contains("tenant-paused"), "{text}");
    assert!(text.contains("tenant-resumed"), "{text}");

    // A failed provision, released.
    tenants::reserve(&h.pool, "spc_failed_dt", "Failed", "x", NOW, None)
        .await
        .unwrap();
    tenants::fail(&h.pool, "spc_failed_dt", "stopped", NOW)
        .await
        .unwrap();
    let (status, body) = command(
        &h,
        reap_op::REAP,
        json!({"tenant": "spc_failed_dt"}),
        "dt-reap",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let text = records(&h);
    assert!(text.contains("space_reaped"), "{text}");
    assert!(text.contains(&format!("by={DEPLOYMENT_OWNER}")), "{text}");

    // The new tenant is in the list, with its owner counted.
    let (_, body) = list(&h, HOME, DEPLOYMENT_OWNER, "").await;
    assert_eq!(row(&body, &id)["status"], "active");
    assert_eq!(row(&body, &id)["name"], "Recorded");
}
