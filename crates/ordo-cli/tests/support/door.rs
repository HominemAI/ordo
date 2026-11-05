//! A deployment's door, listening on a loopback port in this process, for
//! the tests of commands that go through it (owner, 2026-11-05: "the CLI
//! goes through the door"). Shared by `wire_apply.rs` and
//! `schema_apply.rs`.

use std::sync::Arc;

use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::apply_op;
use ordo_host::grants::PgGrantRegistry;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::FixtureExecutor;
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};

pub const NOW: u64 = 1_000_000;
pub const SECRET: &[u8] = b"wire-apply-secret";
pub const HOME: &str = "ordo_home";
pub const DEP_OWNER: &str = "dep_owner";
pub const DEP_MANAGER: &str = "dep_manager";

pub fn token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            // Present, so step-up is satisfied. Every platform op requires
            // it; see `wire.rs`'s module doc on where an operator's token
            // comes from.
            user_presence_at: Some(IStamp(NOW)),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

/// The deployment, listening on a loopback port.
///
/// A real socket rather than `oneshot` on the router: the point of this
/// file is the **client**, and a client that was handed a `Service` would
/// prove nothing about the request it builds, the headers it sends or the
/// body it reads back off the wire.
pub async fn serve(target_db: &str) -> std::net::SocketAddr {
    let f = Arc::new(Fixture::load().expect("fixture"));

    let dir_db = format!("{target_db}_dir");
    let dir_pg = Pg::named(&dir_db);
    f.reset(&dir_pg).expect("seeded");
    let dir_pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: dir_db,
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("directory pool");

    let target = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: target_db.to_string(),
        size: 3,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("target pool");

    let grants = Arc::new(PgGrantRegistry::new(dir_pool.clone()));
    grants.ensure_table().await.expect("platform table");

    // The same rows, through the same registration loop a deployment uses.
    let row = |id: &str, role: &str| {
        vec![
            Some(HOME.to_string()),
            Some(id.to_string()),
            Some(role.to_string()),
        ]
    };
    let engine = Arc::new(
        ordo_host::engine_from_members(&f, &[row(DEP_OWNER, "owner"), row(DEP_MANAGER, "manager")])
            .expect("engine"),
    );

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "wire-apply-test",
        f.clone(),
        ordo_exec::Executor {
            pool: dir_pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        platform_ops::specs(),
    )
    .expect("platform specs");

    let ops = Arc::new(
        PlatformOps::new(engine.clone(), grants, Arc::new(|| NOW))
            .with_apply(apply_op::ApplyOps::new(
                target.clone(),
                std::sync::Arc::new(ordo_exec::route::SinglePool::new(target.clone())),
                HOME,
            ))
            .with_tenant_list(Arc::new(ordo_host::fleet::TenantListOps {
                control: target.clone(),
                home: HOME.to_string(),
                spaces: None,
            }))
            .with_quotas(Arc::new(ordo_host::quotas::QuotaOps {
                control: target,
                home: HOME.to_string(),
                enforced: false,
                stream: None,
                now: Arc::new(|| NOW),
            })),
    );
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: dir_pg.clone(),
                page_size: 50,
            }),
            ops,
        )),
        access: Arc::new(ordo_host::IdentityAccess::sharing(
            engine,
            TokenKey::from_secret(SECRET),
            Box::new(|| NOW),
            Arc::new(ordo_host::metrics::Metrics::new()),
        )),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"wire-apply-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    addr
}

/// A runtime the server lives on, held for the length of the test. The CLI
/// runs outside it, on the test's own thread, and builds its own.
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// The door flags every `--host` command here takes.
pub fn door(addr: std::net::SocketAddr, tok: &str) -> Vec<String> {
    vec![
        "--host".into(),
        format!("http://{addr}"),
        "--token".into(),
        tok.into(),
        "--space".into(),
        HOME.into(),
    ]
}
