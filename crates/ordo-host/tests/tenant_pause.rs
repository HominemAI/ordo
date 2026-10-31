//! **Pausing a tenant** (checklist 27ar, `plan-tenant-pause.md`, owner rulings
//! of 2026-10-05), through the real door against real Postgres.
//!
//! The claims, in the order they matter:
//!
//! - a paused tenant refuses its ops with `space_paused` (423), and a resume
//!   lifts it;
//! - an op the app marks `nopause` keeps answering, and so do the platform
//!   ops that let people look, get out, or take access away;
//! - a caller with no membership learns nothing about a pause;
//! - pausing is the deployment owner's by default, and the app developer may
//!   grant it to tenant roles; a tenant cannot lift the deployment's pause;
//! - the record is one row, written once, with a reason from a closed list;
//! - every replica learns of a pause from the registry.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, BoundParams, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::member_ops::{self, MemberOps};
use ordo_host::pause::{self, PauseAnswers, PauseOps, PausePolicy};
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::runtime::SwappableAccess;
use ordo_host::tenants::{self, State};
use ordo_host::FixtureExecutor;
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: [u8; 32] = *b"tenant-pause-test-secret-32bytes";

// The conformance seed: mem_o owner, mem_1 manager, mem_2 and mem_3 editors,
// mem_4 viewer, all in the tenant `conformance`.
const OWNER: &str = "mem_o";
const EDITOR: &str = "mem_2";
const VIEWER: &str = "mem_4";
/// The deployment owner, an owner of the deployment's own space only.
const DEPLOYMENT_OWNER: &str = "dep_owner";
const HOME: &str = "home";

/// An op the app marks to keep answering while paused.
const NOPAUSE_OP: &str = "
op pausedCatalog nopause unbounded =
  products(active == true)[name, price]
";

struct Harness {
    access: Arc<SwappableAccess>,
    state: ApiState,
    pool: ordo_exec::Pool,
    spaces: Arc<ordo_host::spaces::Spaces>,
    deployment: Arc<ordo_host::deploy::Deployment>,
    f: Arc<Fixture>,
    space: String,
}

async fn harness(db: &str, pausing: PausePolicy) -> Harness {
    let source = format!("{FIXTURE}\n{NOPAUSE_OP}\n");
    let f = Arc::new(Fixture::from_source(&source, SEED).expect("fixture"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 3,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    let space = ordo_conformance::fixture_space().0;

    // The tenant is one this deployment minted: a registry row, active.
    tenants::ensure_table(&pool).await.expect("registry");
    tenants::reserve(&pool, &space, "Conformance", OWNER, NOW, None)
        .await
        .expect("reserved");
    assert!(tenants::activate(&pool, &space, NOW).await.expect("active"));

    let grants = Arc::new(PgGrantRegistry::new(pool.clone()));
    grants.ensure_table().await.expect("platform table");

    // The tenant's members, and the deployment owner in the deployment's
    // own space.
    let mut rows = pool.rows(ordo_host::MEMBERS_SQL).await.expect("members");
    rows.push(vec![
        Some(HOME.into()),
        Some(DEPLOYMENT_OWNER.into()),
        Some("owner".into()),
        Some("active".into()),
        Some(DEPLOYMENT_OWNER.into()),
    ]);
    let engine = ordo_host::engine_from_members_with(
        &f,
        &rows,
        &ordo_host::presence::PresencePolicy::everything_required(),
        &pausing,
    )
    .expect("engine");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let door = ordo_host::IdentityAccess::sharing(
        Arc::new(engine),
        TokenKey::from_secret(&SECRET),
        Box::new(ordo_host::runtime::now_secs),
        metrics.clone(),
    );
    let access = Arc::new(SwappableAccess::new(door));

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "tenant-pause-test",
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

    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());
    spaces.set_pause_answers(pausing.answers().clone());
    pause::reload(&spaces, &pool).await.expect("register");
    let members = Arc::new(MemberOps {
        control: pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
        directory: None,
        stream: Some(access.recorder.clone()),
        refresh: None::<member_ops::Refresh>,
        now: Arc::new(|| NOW),
        managed: None,
    });
    let live = access.clone();
    let ops = Arc::new(
        PlatformOps::new(
            access.current().engine.clone(),
            grants.clone(),
            Arc::new(|| NOW),
        )
        .with_engine_source(Arc::new(move || live.current().engine.clone()))
        .with_members(members)
        .with_tenants(ordo_host::tenant_ops::TenantOps::new(
            pool.clone(),
            f.keys.clone(),
            HOME,
        ))
        .with_pause(Arc::new(PauseOps {
            control: pool.clone(),
            spaces: spaces.clone(),
            home: HOME.into(),
            stream: Some(access.recorder.clone()),
            now: Arc::new(|| NOW),
        })),
    );

    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: spaces.clone(),
                pg: pg.clone(),
                page_size: 50,
            }),
            ops,
        )),
        access: access.clone(),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"tenant-pause-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        access,
        state,
        pool,
        spaces,
        deployment,
        f,
        space,
    }
}

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

async fn send(state: &ApiState, req: Request<Body>) -> (StatusCode, Value) {
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A write-kind op, bound to `space`.
async fn command(
    h: &Harness,
    space: &str,
    op: &str,
    who: &str,
    body: Value,
) -> (StatusCode, Value) {
    let mut body = body;
    body["$space"] = json!(space);
    send(
        &h.state,
        Request::builder()
            .method("POST")
            .uri(format!("/op/{op}"))
            .header("x-ordo-space", space)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", token(who)))
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

/// A read op in the tenant.
async fn read(h: &Harness, op: &str, who: &str) -> (StatusCode, Value) {
    send(
        &h.state,
        Request::builder()
            .method("GET")
            .uri(format!("/op/{op}"))
            .header("x-ordo-space", &h.space)
            .header("authorization", format!("Bearer {}", token(who)))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn pause_by_deployment(h: &Harness, note: &str) -> (StatusCode, Value) {
    command(
        h,
        HOME,
        pause::PAUSE_TENANT,
        DEPLOYMENT_OWNER,
        json!({ "tenant": h.space, "note": note }),
    )
    .await
}

async fn resume_by_deployment(h: &Harness) -> (StatusCode, Value) {
    command(
        h,
        HOME,
        pause::RESUME_TENANT,
        DEPLOYMENT_OWNER,
        json!({ "tenant": h.space }),
    )
    .await
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

fn message(v: &Value) -> String {
    v["error"]["message"].as_str().unwrap_or("").to_string()
}

// ── the hold ─────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_tenant_refuses_app_ops_with_space_paused_and_a_resume_lifts_it() {
    let h = harness("ordo_pause_refuses", PausePolicy::deployment_only()).await;
    let (status, _) = read(&h, "activeProducts", EDITOR).await;
    assert_eq!(status, StatusCode::OK, "the control: it serves first");

    let (status, body) = pause_by_deployment(&h, "billing").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["state"], "paused");
    assert_eq!(body["rows"][0]["pausedBy"], "deployment");
    assert_eq!(body["rows"][0]["note"], "billing");
    assert_eq!(body["rows"][0]["outcome"], "paused");

    let (status, body) = read(&h, "activeProducts", EDITOR).await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
    assert_eq!(code(&body), "space_paused");
    assert!(message(&body).contains("by the deployment"), "{body}");
    // Derived std ops carry no flag, and a write is refused like a read.
    let (status, body) = command(
        &h,
        &h.space.clone(),
        "std.categories.create",
        OWNER,
        json!({ "name": "new" }),
    )
    .await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");

    let (status, body) = resume_by_deployment(&h).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["state"], "active");
    assert_eq!(body["rows"][0]["outcome"], "resumed");
    let (status, body) = read(&h, "activeProducts", EDITOR).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The owner, 2026-10-06: what a paused tenant answers is the app
/// developer's, per reason.
#[tokio::test(flavor = "multi_thread")]
async fn a_paused_tenant_answers_423_with_todays_message_unless_the_app_developer_set_otherwise() {
    let h = harness("ordo_pause_default_answer", PausePolicy::deployment_only()).await;
    pause_by_deployment(&h, "billing").await;
    let (status, body) = read(&h, "activeProducts", EDITOR).await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
    assert_eq!(code(&body), "space_paused");
    let m = message(&body);
    assert!(
        m.starts_with(
            "space_paused: this tenant is paused (by the deployment, reason `billing`, since "
        ),
        "{m}"
    );
    assert!(
        m.ends_with("Nothing ran. It answers again once it is resumed"),
        "{m}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_billing_pause_answers_the_status_and_message_the_app_developer_set() {
    let answers = PauseAnswers::parse(
        r#"{"billing": {"status": 402, "message": "Your plan is unpaid. Pay the invoice to continue."},
            "abuse": {"status": 503}}"#,
    )
    .expect("answers");
    let policy = PausePolicy::deployment_only().with_answers(answers);
    let h = harness("ordo_pause_billing_answer", policy).await;

    pause_by_deployment(&h, "billing").await;
    let (status, body) = read(&h, "activeProducts", EDITOR).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(code(&body), "space_paused", "the class never changes");
    assert_eq!(
        message(&body),
        "Your plan is unpaid. Pay the invoice to continue."
    );
    // A write answers the same.
    let (status, body) = command(
        &h,
        &h.space.clone(),
        "std.categories.create",
        OWNER,
        json!({ "name": "new" }),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    resume_by_deployment(&h).await;

    // A reason with a status and no message keeps today's message, and a 503
    // is still the same class.
    pause_by_deployment(&h, "abuse").await;
    let (status, body) = read(&h, "activeProducts", EDITOR).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(code(&body), "space_paused");
    assert!(message(&body).contains("reason `abuse`"), "{body}");
    resume_by_deployment(&h).await;

    // A reason the app developer left alone answers 423.
    pause_by_deployment(&h, "legal").await;
    let (status, body) = read(&h, "activeProducts", EDITOR).await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_op_the_app_marks_nopause_keeps_answering_while_paused() {
    let h = harness("ordo_pause_nopause", PausePolicy::deployment_only()).await;
    let (status, body) = pause_by_deployment(&h, "billing").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = read(&h, "pausedCatalog", VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body["rows"].as_array().expect("rows").is_empty(), "{body}");
    let (status, _) = read(&h, "activeProducts", VIEWER).await;
    assert_eq!(status, StatusCode::LOCKED);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_caller_with_no_membership_is_told_access_denied_and_learns_nothing_of_the_pause() {
    let h = harness("ordo_pause_stranger", PausePolicy::deployment_only()).await;
    pause_by_deployment(&h, "abuse").await;
    let (status, body) = read(&h, "activeProducts", "stranger").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_member_commands_answer_while_paused_and_sharing_does_not() {
    let h = harness("ordo_pause_members", PausePolicy::deployment_only()).await;
    pause_by_deployment(&h, "legal").await;

    // An owner can still suspend a departing employee.
    let space = h.space.clone();
    let (status, body) = command(
        &h,
        &space,
        member_ops::SUSPEND,
        OWNER,
        json!({ "member": VIEWER }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["status"], "suspended");

    // Nothing new: no admission while paused.
    let (status, body) = command(
        &h,
        &space,
        member_ops::ADMIT,
        OWNER,
        json!({ "principal": "newcomer", "role": "viewer" }),
    )
    .await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
    assert_eq!(code(&body), "space_paused");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tenant_list_shows_a_paused_tenant_with_who_paused_it_and_why() {
    let h = harness("ordo_pause_list", PausePolicy::deployment_only()).await;
    let space = h.space.clone();
    let list = |h: &Harness| {
        let space = space.clone();
        let state = h.state.clone();
        async move {
            send(
                &state,
                Request::builder()
                    .method("GET")
                    .uri(format!("/op/{}", platform_ops::MY_TENANTS))
                    .header("x-ordo-space", &space)
                    .header("authorization", format!("Bearer {}", token(EDITOR)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
        }
    };
    let (status, body) = list(&h).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["state"], "active", "{body}");

    pause_by_deployment(&h, "maintenance").await;
    let (status, body) = list(&h).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the list answers on a paused tenant: {body}"
    );
    let row = &body["rows"][0];
    assert_eq!(row["state"], "paused", "{body}");
    assert_eq!(row["pausedBy"], "deployment");
    assert_eq!(row["note"], "maintenance");
    assert_eq!(row["pausedAt"], NOW);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_tenant_admits_nobody_new_and_admits_them_after_a_resume() {
    use ordo_host::admission::{self, Admission, Arrival, Ceiling, Outcome, Policy, PolicyStore};
    let h = harness("ordo_pause_arrival", PausePolicy::deployment_only()).await;
    admission::ensure_table(&h.pool).await.expect("table");
    let store = PolicyStore::new(h.pool.clone());
    let open = Policy {
        open: true,
        ..Policy::default()
    };
    store.put(&h.space, &open, OWNER, NOW).await.expect("open");
    let gate = Admission {
        members: Arc::new(MemberOps {
            control: h.pool.clone(),
            pools: Arc::new(ordo_exec::route::SinglePool::new(h.pool.clone())),
            directory: None,
            stream: None,
            refresh: None,
            now: Arc::new(|| NOW),
            managed: None,
        }),
        store,
        tickets: None,
        ceiling: Ceiling::everything(),
        home: None,
    };
    let arrival = Arrival {
        ticket: None,
        provider: None,
    };

    pause_by_deployment(&h, "billing").await;
    let out = gate
        .on_arrival("newcomer", &h.space, &arrival)
        .await
        .expect("answers");
    assert!(matches!(out, Outcome::Refused), "{out:?}");

    resume_by_deployment(&h).await;
    let out = gate
        .on_arrival("newcomer", &h.space, &arrival)
        .await
        .expect("answers");
    assert!(matches!(out, Outcome::Admitted { .. }), "{out:?}");
}

// ── who may ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn by_default_a_tenant_owner_cannot_pause_their_own_tenant() {
    let h = harness("ordo_pause_default", PausePolicy::deployment_only()).await;
    let space = h.space.clone();
    let (status, body) = command(&h, &space, pause::PAUSE, OWNER, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied");
    let t = tenants::lookup(&h.pool, &h.space).await.unwrap().unwrap();
    assert_eq!(t.state, State::Active);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_granted_role_pauses_and_resumes_and_a_deployment_pause_is_beyond_it() {
    let policy = PausePolicy::parse(Some("owner"), None).expect("policy");
    let h = harness("ordo_pause_granted", policy).await;
    let space = h.space.clone();

    // The grant reaches the owner and nobody else.
    let (status, _) = command(&h, &space, pause::PAUSE, EDITOR, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = command(&h, &space, pause::PAUSE, OWNER, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["pausedBy"], "tenant");
    assert_eq!(body["rows"][0]["note"], "owner", "the default reason");
    // Resume answers on a paused tenant.
    let (status, body) = command(&h, &space, pause::RESUME, OWNER, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["outcome"], "resumed");

    // A deployment pause is lifted only by the deployment owner.
    pause_by_deployment(&h, "billing").await;
    let (status, body) = command(&h, &space, pause::RESUME, OWNER, json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(code(&body), "tenant_refused");
    assert!(message(&body).starts_with("paused_by_deployment"), "{body}");
    let (status, body) = resume_by_deployment(&h).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_deployment_resumes_a_pause_the_tenant_made() {
    let policy = PausePolicy::parse(Some("owner"), None).expect("policy");
    let h = harness("ordo_pause_dep_lifts", policy).await;
    let space = h.space.clone();
    let (status, _) = command(&h, &space, pause::PAUSE, OWNER, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = resume_by_deployment(&h).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["outcome"], "resumed");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_deployments_ops_answer_only_in_its_own_space_and_cannot_pause_it() {
    let h = harness("ordo_pause_home", PausePolicy::deployment_only()).await;
    // Bound to the tenant, the deployment's op is not there for its owner.
    let space = h.space.clone();
    let (status, body) = command(
        &h,
        &space,
        pause::PAUSE_TENANT,
        OWNER,
        json!({ "tenant": space }),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");

    let (status, body) = command(
        &h,
        HOME,
        pause::PAUSE_TENANT,
        DEPLOYMENT_OWNER,
        json!({ "tenant": HOME }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(message(&body).starts_with("home_space"), "{body}");
}

// ── the record ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn two_pauses_record_once_and_the_second_reads_the_first() {
    let h = harness("ordo_pause_twice", PausePolicy::deployment_only()).await;
    let (_, first) = pause_by_deployment(&h, "billing").await;
    assert_eq!(first["rows"][0]["outcome"], "paused");
    let (status, second) = pause_by_deployment(&h, "abuse").await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["rows"][0]["outcome"], "already paused");
    assert_eq!(
        second["rows"][0]["note"], "billing",
        "the first record stands"
    );

    let events = format!("{:?}", h.access.current().take_events());
    // One pause, mirrored once into each of the two streams.
    assert_eq!(
        events
            .matches("tenant=conformance by=deployment note=billing")
            .count(),
        2,
        "{events}"
    );
    assert!(
        !events.contains("note=abuse"),
        "the second wrote nothing: {events}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_reason_is_a_word_from_the_closed_list_never_free_text() {
    let h = harness("ordo_pause_note", PausePolicy::deployment_only()).await;
    let (status, body) = pause_by_deployment(&h, "Ada did not pay").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(message(&body).starts_with("pause_note"), "{body}");
    let t = tenants::lookup(&h.pool, &h.space).await.unwrap().unwrap();
    assert_eq!(t.state, State::Active, "nothing was paused");
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_and_resume_are_recorded_with_the_layer_and_the_reason() {
    let h = harness("ordo_pause_recorded", PausePolicy::deployment_only()).await;
    pause_by_deployment(&h, "legal").await;
    resume_by_deployment(&h).await;
    let events = h.access.current().take_events();
    let text = format!("{events:?}");
    assert!(text.contains("tenant-paused"), "{text}");
    assert!(text.contains("tenant-resumed"), "{text}");
    assert!(text.contains("by=deployment note=legal"), "{text}");
    // Both streams: the deployment's own, and the tenant's.
    assert_eq!(events.len(), 4, "{text}");
    assert_eq!(
        events
            .iter()
            .filter(|e| format!("{:?}", e.scope).starts_with("Space"))
            .count(),
        2,
        "{text}"
    );
}

// ── the state machine ────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn provisioning_and_failed_tenants_cannot_be_paused() {
    let h = harness("ordo_pause_states", PausePolicy::deployment_only()).await;
    tenants::reserve(&h.pool, "spc_building", "Building", OWNER, NOW, None)
        .await
        .unwrap();
    let out = tenants::pause(&h.pool, "spc_building", "deployment", "other", NOW)
        .await
        .unwrap();
    assert!(matches!(out, tenants::Paused::NotServing(_)), "{out:?}");
    tenants::fail(&h.pool, "spc_building", "stopped", NOW)
        .await
        .unwrap();
    let out = tenants::pause(&h.pool, "spc_building", "deployment", "other", NOW)
        .await
        .unwrap();
    assert!(matches!(out, tenants::Paused::NotServing(_)), "{out:?}");
    let out = tenants::pause(&h.pool, "spc_never", "deployment", "other", NOW)
        .await
        .unwrap();
    assert_eq!(out, tenants::Paused::Unknown);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_tenant_is_not_timed_out_and_is_not_trouble() {
    let h = harness("ordo_pause_reconciler", PausePolicy::deployment_only()).await;
    pause_by_deployment(&h, "maintenance").await;
    let timed_out = tenants::time_out_stale(&h.pool, 1, NOW + 10_000)
        .await
        .unwrap();
    assert_eq!(timed_out, 0);
    let t = tenants::lookup(&h.pool, &h.space).await.unwrap().unwrap();
    assert_eq!(t.state, State::Paused);
    assert!(tenants::troubled(&h.pool)
        .await
        .unwrap()
        .iter()
        .all(|t| t.id != h.space));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_own_tenant_still_holds_the_one_tenant_latch() {
    let h = harness("ordo_pause_latch", PausePolicy::deployment_only()).await;
    assert!(
        tenants::reserve(&h.pool, "spc_ada", "Ada", "ada", NOW, Some("ada"))
            .await
            .unwrap()
    );
    assert!(tenants::activate(&h.pool, "spc_ada", NOW).await.unwrap());
    tenants::pause(&h.pool, "spc_ada", "deployment", "billing", NOW)
        .await
        .unwrap();
    let own = tenants::own_of(&h.pool, "ada")
        .await
        .unwrap()
        .expect("held");
    assert_eq!(own.id, "spc_ada");
    assert_eq!(own.state, State::Paused);
    assert!(
        !tenants::reserve(&h.pool, "spc_ada2", "Ada again", "ada", NOW, Some("ada"))
            .await
            .unwrap(),
        "a second own tenant must not be minted while the first is paused"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reaping_refuses_a_paused_tenant() {
    let h = harness("ordo_pause_reap", PausePolicy::deployment_only()).await;
    pause_by_deployment(&h, "abuse").await;
    let reap = ordo_host::reap_op::ReapOps {
        pool: h.pool.clone(),
        keys: h.f.keys.clone(),
        home: HOME.into(),
        credentials: Arc::new(ordo_exec::route::Installed::new().as_configured("local")),
        template: ordo_exec::PoolConfig::from_env(),
        stream: None,
        now: Arc::new(|| NOW),
    };
    let mut bound = serde_json::Map::new();
    bound.insert("tenant".into(), json!(h.space));
    let e = reap
        .reap(HOME, &BoundParams(bound), DEPLOYMENT_OWNER)
        .await
        .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::TenantRefused, "{e}");
    assert!(e.message.contains("active or paused"), "{e}");
}

// ── every replica ────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn another_replica_learns_a_pause_and_a_resume_from_the_registry() {
    let h = harness("ordo_pause_replicas", PausePolicy::deployment_only()).await;
    // A second replica: its own resolver over the same deployment.
    let other = ordo_host::spaces::Spaces::new(h.deployment.clone());
    assert!(pause::gate(&other, "activeProducts", &h.space).is_ok());

    pause_by_deployment(&h, "billing").await;
    assert!(
        pause::gate(&h.spaces, "activeProducts", &h.space).is_err(),
        "held here at once"
    );
    assert_eq!(pause::reload(&other, &h.pool).await.unwrap(), 1);
    let e = pause::gate(&other, "activeProducts", &h.space).unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::SpacePaused);
    assert!(pause::gate(&other, "pausedCatalog", &h.space).is_ok());

    resume_by_deployment(&h).await;
    assert_eq!(pause::reload(&other, &h.pool).await.unwrap(), 0);
    assert!(pause::gate(&other, "activeProducts", &h.space).is_ok());
}
