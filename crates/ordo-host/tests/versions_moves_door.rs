//! **Who may move a tenant off a revoked version, through the real door**
//! (`plan-versions.md` §1.11, owner rulings of 2026-11-05):
//!
//! - `std.space.moveOffRevoked`, bound to the tenant, is refused to every
//!   tenant role by default, and admitted to the roles
//!   `ORDO_VERSIONS_TENANT_MOVE_ROLES` names, and to no other;
//! - `std.deployment.moveOffRevoked` is the deployment owner's: a manager
//!   of the deployment's own space is refused.
//!
//! The tenant policy is installed once per process, so this file holds one
//! test, which builds the door before the grant and again after it.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_exec::route::SinglePool;
use ordo_host::grants::PgGrantRegistry;
use ordo_host::pause::PausePolicy;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::versions::Versions;
use ordo_host::FixtureExecutor;
use ordo_host::{apply_op, deploy_ops, revoked_moves, tenants};
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: [u8; 32] = *b"versions-moves-door-secret-32byt";
/// The conformance seed: `mem_o` owns the tenant, `mem_2` edits it.
const TENANT_OWNER: &str = "mem_o";
const EDITOR: &str = "mem_2";
const DEPLOYMENT_OWNER: &str = "dep_owner";
/// A manager of the deployment's own space: not its owner.
const MANAGER: &str = "dep_manager";
const HOME: &str = "home";

/// The door, built under whatever move policy is installed now.
async fn door(db: &str) -> (ApiState, String) {
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
    Versions::on(&pool).ensure().await.expect("version tables");
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
        "versions-moves-door",
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
    let ops = Arc::new(
        PlatformOps::new(engine, grants, Arc::new(|| NOW)).with_apply(
            apply_op::ApplyOps::recording(
                pool.clone(),
                Arc::new(SinglePool::new(pool.clone())),
                HOME,
                None,
            ),
        ),
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
        page_key: PageKey::from_secret(b"versions-moves-door"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    (state, space)
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

/// A write-kind op as `who`, bound to `space`.
async fn command(
    state: &ApiState,
    space: &str,
    op: &str,
    who: &str,
    body: Value,
) -> (StatusCode, Value) {
    let mut body = body;
    body["$space"] = json!(space);
    let req = Request::builder()
        .method("POST")
        .uri(format!("/op/{op}"))
        .header("x-ordo-space", space)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token(who)))
        .body(Body::from(body.to_string()))
        .unwrap();
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

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

/// Admitted by the door: the op ran and found the tenant on no revoked
/// version, which is what the conformance tenant is on.
fn admitted(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("not_on_revoked"),
        "{body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_door_admits_a_tenant_move_only_when_granted_and_the_deployment_move_to_its_owner() {
    // ── by default ──
    let (state, space) = door("ordo_vmove_door_default").await;
    for who in [TENANT_OWNER, EDITOR] {
        let (status, body) =
            command(&state, &space, revoked_moves::MOVE_SELF, who, json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}: {body}");
        assert_eq!(code(&body), "access_denied", "{who}: {body}");
    }

    // The deployment's move: its owner is admitted, a manager of the
    // deployment's own space is not, nor the tenant's owner.
    let tenant = json!({ "tenant": space });
    let (status, body) = command(
        &state,
        HOME,
        deploy_ops::MOVE_OFF_REVOKED,
        DEPLOYMENT_OWNER,
        tenant.clone(),
    )
    .await;
    admitted(status, &body);
    for who in [MANAGER, TENANT_OWNER] {
        let (status, body) = command(
            &state,
            HOME,
            deploy_ops::MOVE_OFF_REVOKED,
            who,
            tenant.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}: {body}");
        assert_eq!(code(&body), "access_denied", "{who}: {body}");
    }

    // ── granted to the tenant's owners ──
    revoked_moves::install(revoked_moves::MovePolicy::parse(Some("owner")).expect("policy"));
    let (state, space) = door("ordo_vmove_door_granted").await;
    let (status, body) = command(
        &state,
        &space,
        revoked_moves::MOVE_SELF,
        TENANT_OWNER,
        json!({}),
    )
    .await;
    admitted(status, &body);
    let (status, body) = command(&state, &space, revoked_moves::MOVE_SELF, EDITOR, json!({})).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the grant is the owner's: {body}"
    );
}
