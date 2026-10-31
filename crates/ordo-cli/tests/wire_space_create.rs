//! `ordo space create --host …` (17aa piece 3), the client half of
//! `std.space.create`, over a real socket.
//!
//! `ordo-host`'s `tenant_create.rs` proves the op through the door: the
//! gate, the mint, and that a refused create leaves nothing behind. This
//! file proves what the CLI does with the same door: the host, token and
//! space resolution `wire.rs` shares with `apply --host`, and that every
//! refusal the op can produce prints a message a human can act on, not
//! just a class.
//!
//! Unlike `wire_apply.rs`, there is no direct-database twin to compare
//! against: `std.space.create` never had one (`tenant_ops.rs`'s module
//! doc), so `ordo space create` is HTTP-only and these tests check its one
//! path against the door's real answers.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
mod support;
use ordo_conformance::{Fixture, Pg};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::tenant_ops;
use ordo_host::FixtureExecutor;
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use support::operator;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"wire-space-create-secret";
const HOME: &str = "ordo_home";
/// A tenant that already exists. Its owner holds `Role::Owner` there,
/// which is exactly the authority check 3 of the gate must not accept.
const TENANT: &str = "tenant_a";

const DEP_OWNER: &str = "dep_owner";
const DEP_MANAGER: &str = "dep_manager";
const TENANT_OWNER: &str = "ten_owner";
const CUSTOMER: &str = "customer_ada";

fn token(principal: &str) -> String {
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

/// The deployment, listening on a loopback port, wired with `TenantOps`
/// the way a real boot wires it (`with_tenants`) rather than a `oneshot`
/// call into the router: the point of this file is what the CLI sends and
/// reads off the wire.
async fn serve(db: &str) -> std::net::SocketAddr {
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

    // What the boot does before serving; the op reads and writes it.
    ordo_host::tenants::ensure_table(&pool)
        .await
        .expect("$spaces");

    let grants = Arc::new(PgGrantRegistry::new(pool.clone()));
    grants.ensure_table().await.expect("platform table");

    let row = |space: &str, id: &str, role: &str| {
        vec![
            Some(space.to_string()),
            Some(id.to_string()),
            Some(role.to_string()),
        ]
    };
    let engine = Arc::new(
        ordo_host::engine_from_members(
            &f,
            &[
                row(HOME, DEP_OWNER, "owner"),
                row(HOME, DEP_MANAGER, "manager"),
                row(TENANT, TENANT_OWNER, "owner"),
            ],
        )
        .expect("engine"),
    );

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "wire-space-create-test",
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
    .expect("platform specs");

    let ops = Arc::new(
        PlatformOps::new(engine.clone(), grants, Arc::new(|| NOW)).with_tenants(
            // Routed, because every tenant has a database (ruled
            // 2026-09-25) and a deployment that cannot create one cannot
            // create a tenant. The default cluster is the conformance
            // cluster, which is also this harness's control cluster: the
            // zero-configuration case.
            tenant_ops::TenantOps::with_routing(
                pool.clone(),
                f.keys.clone(),
                HOME,
                tenant_ops::Routing {
                    serving: deployment.clone(),
                    credentials: ordo_exec::route::WithOwnCredential::wrapping(
                        Arc::new(ordo_exec::route::Installed::new().as_configured("local")),
                        ordo_exec::PoolConfig::from_env().password.clone(),
                    ),
                    template: ordo_exec::PoolConfig {
                        size: 2,
                        ..ordo_exec::PoolConfig::from_env()
                    },
                    hosts: ordo_host::provision::HostAllowlist::of("")
                        .allowing(&ordo_exec::PoolConfig::from_env().host),
                    default_cluster: tenant_ops::TenantCluster::from_env(
                        &ordo_exec::PoolConfig::from_env(),
                    ),
                    stream: None,
                },
            ),
        ),
    );
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: pg.clone(),
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
        page_key: PageKey::from_secret(b"wire-space-create-test"),
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
/// runs outside it, on the test's own thread, and builds its own, the
/// same split `wire_apply.rs` uses for the same reason.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

fn create(
    addr: std::net::SocketAddr,
    tok: &str,
    bound_space: &str,
    name: &str,
    owner: &str,
) -> operator::Outcome {
    operator::run(
        "space",
        &[
            "create".into(),
            "--name".into(),
            name.into(),
            "--owner".into(),
            owner.into(),
            "--host".into(),
            format!("http://{addr}"),
            "--token".into(),
            tok.into(),
            "--space".into(),
            bound_space.into(),
        ],
    )
}

// ── the happy path ──────────────────────────────────────────────────────

/// A deployment owner mints a tenant over the wire, and the CLI prints the
/// id the door minted, the name and owner it was given back, and that the
/// tenant is reachable right now.
#[test]
fn a_deployment_owner_mints_a_tenant_and_the_cli_prints_it() {
    let rt = runtime();
    let addr = rt.block_on(serve("ordo_wire_space_owner"));

    let out = create(addr, &token(DEP_OWNER), HOME, "Acme Corp", CUSTOMER);
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(
        out.report.contains("space      spc_"),
        "an id the platform minted: {}",
        out.report
    );
    assert!(
        out.report.contains("name       Acme Corp"),
        "{}",
        out.report
    );
    assert!(
        out.report.contains("owner      customer_ada"),
        "{}",
        out.report
    );
    assert!(
        out.report.contains("reachable  now"),
        "the mint announces itself; the door should already reach it: {}",
        out.report
    );
}

// ── the gate ─────────────────────────────────────────────────────────────

/// Check 3. A tenant's own owner is bound to their own space, not the
/// deployment's, and cannot mint the deployment's next tenant. The CLI
/// prints the door's class by name.
#[test]
fn a_tenants_owner_is_refused_not_found() {
    let rt = runtime();
    let addr = rt.block_on(serve("ordo_wire_space_tenant_owner"));

    let out = create(addr, &token(TENANT_OWNER), TENANT, "Theirs", "eve");
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("[not_found]"), "{}", out.report);
}

/// Check 1. A manager administers who may do what inside the deployment's
/// own space; deciding who the deployment serves next is not that.
#[test]
fn a_deployment_manager_is_refused_access_denied() {
    let rt = runtime();
    let addr = rt.block_on(serve("ordo_wire_space_manager"));

    let out = create(addr, &token(DEP_MANAGER), HOME, "Theirs", "eve");
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("[access_denied]"), "{}", out.report);
}

// ── what a caller can get wrong, and can tell from the printed message ───

/// A blank name is refused, and the printed message names the rule: a
/// tenant whose id is unreadable by design may not go nameless too.
#[test]
fn a_blank_name_is_refused_and_the_message_names_the_rule() {
    let rt = runtime();
    let addr = rt.block_on(serve("ordo_wire_space_blankname"));

    let out = create(addr, &token(DEP_OWNER), HOME, "   ", CUSTOMER);
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("[tenant_refused]"), "{}", out.report);
    assert!(
        out.report.contains("may not be blank"),
        "the printed message should say WHICH rule was broken: {}",
        out.report
    );
}

/// An owner the door could never address is refused, and the printed
/// message names the rule too, and it is a DIFFERENT message than a
/// blank name's. A caller must be able to tell the two apart from the
/// text alone.
#[test]
fn an_owner_the_door_cannot_address_is_refused_and_the_message_names_the_rule() {
    let rt = runtime();
    let addr = rt.block_on(serve("ordo_wire_space_badowner"));

    let out = create(addr, &token(DEP_OWNER), HOME, "Acme", "user:ada");
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("[tenant_refused]"), "{}", out.report);
    assert!(
        out.report.contains("letters, digits"),
        "the printed message should name the rule an owner must satisfy: {}",
        out.report
    );
}

// ── what the client refuses before it sends anything ─────────────────────

/// This command has no database mode: without `--host` (or `ORDO_HOST`)
/// it is a usage error that says so, and opens no connection.
#[test]
fn without_a_host_the_command_says_there_is_no_database_mode() {
    let out = operator::run(
        "space",
        &[
            "create".into(),
            "--name".into(),
            "Acme".into(),
            "--owner".into(),
            CUSTOMER.into(),
            "--token".into(),
            "t".into(),
            "--space".into(),
            HOME.into(),
        ],
    );
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("no database mode"), "{}", out.report);
}

/// `--name` is required, and a missing one is a usage error naming the
/// flag, before a connection is opened.
#[test]
fn a_missing_name_is_a_usage_error() {
    let out = operator::run(
        "space",
        &[
            "create".into(),
            "--owner".into(),
            CUSTOMER.into(),
            "--host".into(),
            "http://127.0.0.1:1".into(),
            "--token".into(),
            "t".into(),
            "--space".into(),
            HOME.into(),
        ],
    );
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--name"), "{}", out.report);
}

/// An unknown `ordo space` subcommand is a usage error naming the one
/// that exists.
#[test]
fn an_unknown_space_subcommand_is_a_usage_error() {
    let out = operator::run("space", &["reap".into()]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("create"), "{}", out.report);
}
