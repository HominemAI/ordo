//! 17aa piece 3 — `std.space.create` through the real door.
//!
//! The checklist's done-when, in three clauses: *a deployment with a
//! program can mint a tenant through the door, the tenant serves, and
//! nothing anywhere keys on its name.* Each is a test here.
//!
//! # Why the gate gets as much of the file as the happy path
//!
//! This op decides **who the deployment serves**, which is the same class
//! of authority as `std.deployment.apply` and is tested on the same terms
//! (`deploy_apply.rs`): a test that only proved an owner can mint would
//! pass for a build where every tenant's owner could mint tenants into the
//! deployment that hosts them.
//!
//! # The one property that is easy to get wrong, and was
//!
//! `crate::engine_from_members` builds the door's memberships with
//! `members.id` as the **principal**, ignoring the `principal` column
//! beside it. So the founding member id has to *be* the owner principal.
//! The first version of this op minted one, which would have produced a
//! tenant reachable only by an identifier nobody holds a token for. That
//! is what `a_minted_tenant_answers_to_its_owner` pins, and it is the
//! reason it rebuilds the engine from the real rows rather than asserting
//! on the row it just wrote.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::tenant_ops::{self, CREATE};
use ordo_host::tenants;
use ordo_host::FixtureExecutor;
use ordo_identity::principal::{GrantId, PrincipalId, SpaceId as ISpace, Timestamp as IStamp};
use ordo_identity::{AccessEngine, TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"tenant-create-secret";

/// The deployment's own space, and the only one this op answers in.
const HOME: &str = "ordo_home";
/// A tenant that already exists. Its owner holds `Role::Owner` there, which
/// is exactly the authority check 3 of the gate must not accept.
const TENANT: &str = "tenant_a";

const DEP_OWNER: &str = "dep_owner";
const DEP_MANAGER: &str = "dep_manager";
const TENANT_OWNER: &str = "ten_owner";
/// The principal a new tenant is minted *for*. Holds nothing anywhere
/// until one is.
const CUSTOMER: &str = "customer_ada";

struct Harness {
    state: ApiState,
    engine: Arc<AccessEngine>,
    /// The control database: the directory the door is built from, and the
    /// database the new tenant's rows land in.
    pool: ordo_exec::Pool,
    fixture: Arc<Fixture>,
}

fn engine(f: &Fixture) -> AccessEngine {
    let row = |space: &str, id: &str, role: &str| {
        vec![
            Some(space.to_string()),
            Some(id.to_string()),
            Some(role.to_string()),
        ]
    };
    let rows = vec![
        row(HOME, DEP_OWNER, "owner"),
        row(HOME, DEP_MANAGER, "manager"),
        row(TENANT, TENANT_OWNER, "owner"),
    ];
    ordo_host::engine_from_members(f, &rows).expect("engine")
}

/// Every deployment can create tenant databases, because the default tenant
/// cluster defaults to the deployment's own (ruled 2026-09-25). So the
/// ordinary harness routes, and the unrouted one below exists only for the
/// degenerate case `runtime.rs` cannot produce.
async fn harness(db: &str) -> Harness {
    harness_with(db, true).await
}

/// A deployment whose key provider refuses to mint custody for a new
/// space, and is otherwise the ordinary one.
///
/// `bootstrap.md` §4.3 lists the KMS first among the steps that cannot be
/// transactional, and this is the failure it describes arriving on the
/// production path: phase 0 passes, the registry row is reserved, and then
/// phase 2 refuses. It is the only lever in this file that fails a
/// provision **after** the row exists now that every decidable refusal is
/// taken before it — which is the point of that change, and the reason
/// this test double had to be written rather than the old mechanism kept.
async fn harness_whose_keys_refuse(db: &str) -> Harness {
    harness_full(db, true, true, |keys| Arc::new(RefusesNewSpaces(keys))).await
}

/// A deployment whose key provider cannot answer at all — the shape of a KMS
/// that is down, or a process holding the wrong master key.
///
/// Distinct from `harness_whose_keys_refuse`, and the distinction is item
/// 22's whole point: that one can be *read* and will not *create*, so its
/// failure necessarily lands in phase 2 after the registry row exists. This
/// one fails the question preflight asks, so it must be refused before the
/// row exists at all.
async fn harness_whose_custody_is_unreadable(db: &str) -> Harness {
    harness_full(db, true, true, |keys| Arc::new(UnreadableCustody(keys))).await
}

/// A deployment wired with no routing at all. Unreachable from `runtime.rs`,
/// which always builds a default cluster, and kept because the refusal it
/// produces is the one a caller would see if that ever changed.
async fn unrouted_harness(db: &str) -> Harness {
    harness_with(db, false).await
}

async fn harness_with(db: &str, routing: bool) -> Harness {
    harness_full(db, routing, true, |keys| keys).await
}

/// The ordinary deployment with no credential provider configured, which
/// is what a server builds when `ORDO_TENANT_CREDENTIALS` is unset: only
/// `$deployment` answers (ruled 2026-10-06).
async fn harness_without_a_credential_provider(db: &str) -> Harness {
    harness_full(db, true, false, |keys| keys).await
}

/// The credentials a harness dials tenant databases with: the label
/// `local` installed, or, without a provider, what the server builds.
fn harness_credentials(provider: bool) -> Arc<dyn ordo_exec::route::DbCredentials> {
    let own = ordo_exec::PoolConfig::from_env().password.clone();
    match provider {
        true => ordo_exec::route::WithOwnCredential::wrapping(
            Arc::new(ordo_exec::route::Installed::new().as_configured("local")),
            own,
        ),
        false => ordo_host::runtime::tenant_credentials(None, own),
    }
}

async fn harness_full(
    db: &str,
    routing: bool,
    provider: bool,
    wrap_keys: fn(Arc<dyn ordo_crypto::KeyProvider>) -> Arc<dyn ordo_crypto::KeyProvider>,
) -> Harness {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 3,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("control pool");

    // What the boot does before serving; the op reads and writes it.
    tenants::ensure_table(&pool).await.expect("$spaces");
    // The fixture seeds `members` for its own space. Nothing else in this
    // file depends on those rows: the engine above is hand-built, exactly
    // as `deploy_apply.rs` does, because the door must be decided by the
    // production registration loop and not by whatever the seed holds.

    let grants = Arc::new(PgGrantRegistry::new(pool.clone()));
    grants.ensure_table().await.expect("platform table");

    let engine = Arc::new(engine(&f));
    let engine_for_harness = engine.clone();
    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "tenant-create-test",
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

    let tenants = match routing {
        // Every test written before 18d T8: no routing wired, so `$db` is
        // refused rather than half-honoured.
        false => tenant_ops::TenantOps::new(pool.clone(), f.keys.clone(), HOME),
        true => tenant_ops::TenantOps::with_routing(
            pool.clone(),
            wrap_keys(f.keys.clone()),
            HOME,
            tenant_ops::Routing {
                serving: deployment.clone(),
                credentials: harness_credentials(provider),
                template: ordo_exec::PoolConfig {
                    size: 2,
                    ..ordo_exec::PoolConfig::from_env()
                },
                hosts: ordo_host::provision::HostAllowlist::of(
                    &ordo_exec::PoolConfig::from_env().host,
                ),
                // Level 3: this harness's tenants land on the conformance
                // cluster, which is also the control cluster. That is the
                // zero-configuration case and the one most deployments run.
                default_cluster: tenant_ops::TenantCluster::from_env(
                    &ordo_exec::PoolConfig::from_env(),
                ),
                stream: None,
            },
        ),
    };
    let ops = Arc::new(
        PlatformOps::new(engine.clone(), grants, Arc::new(|| NOW))
            .with_tenants(tenants)
            .with_reap(Arc::new(ordo_host::reap_op::ReapOps {
                pool: pool.clone(),
                keys: f.keys.clone(),
                home: HOME.to_string(),
                credentials: ordo_exec::route::WithOwnCredential::wrapping(
                    Arc::new(ordo_exec::route::Installed::new().as_configured("local")),
                    ordo_exec::PoolConfig::from_env().password.clone(),
                ),
                template: ordo_exec::PoolConfig {
                    size: 2,
                    ..ordo_exec::PoolConfig::from_env()
                },
                stream: None,
                now: Arc::new(|| NOW),
            })),
    );
    let access = Arc::new(ordo_host::IdentityAccess::sharing(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW),
        Arc::new(ordo_host::metrics::Metrics::new()),
    ));
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
        access,
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"tenant-create-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        state,
        engine: engine_for_harness,
        pool,
        fixture: f,
    }
}

/// A human's token: present, so step-up is satisfied.
fn token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: Some(IStamp(NOW)),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

/// The strongest capability token that can exist for this op: it names the
/// op and claims fresh presence, and must still be refused.
fn capability_token(engine: &AccessEngine, principal: &str) -> String {
    let set: BTreeSet<String> = BTreeSet::from([CREATE.to_string()]);
    let grant = engine
        .grant_for(
            GrantId("gr_create".into()),
            &ISpace(HOME.to_string()),
            set,
            IStamp(NOW + 3_600),
        )
        .expect("the op is deployed");
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: Some(IStamp(NOW)),
            capability: Some(grant),
            delegation: None,
            session: None,
        },
    )
}

async fn call(
    state: &ApiState,
    space: &str,
    bearer: &str,
    name: &str,
    owner: &str,
) -> (StatusCode, Value) {
    let body = json!({ "$space": space, "name": name, "owner": owner });
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/op/{CREATE}"))
        .header("x-ordo-space", space)
        .header("content-type", "application/json")
        .header("idempotency-key", fresh_key())
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// `call`, plus whatever routing parameters a test wants to send. The
/// existing `call` stays exactly what it was, so the tests that predate
/// this increment keep sending exactly what they sent.
async fn call_with(
    state: &ApiState,
    space: &str,
    bearer: &str,
    name: &str,
    owner: &str,
    extra: Value,
) -> (StatusCode, Value) {
    call_keyed(state, space, bearer, name, owner, extra, Some(&fresh_key())).await
}

/// A key no other call in this binary uses.
///
/// `std.space.create` **requires** `Idempotency-Key` (`bootstrap.md` §2 item
/// 44), so every ordinary call here sends one, which is what a real caller
/// does. Fresh per call rather than fixed: a fixed key would make the second
/// call in any test a replay of the first, and the test would pass by
/// reading a cached answer instead of by doing the thing again.
fn fresh_key() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("tc-{}", N.fetch_add(1, Ordering::Relaxed))
}

/// `call_with`, plus an `Idempotency-Key`. Separate because every test that
/// does not pass one is testing a door that received no key, and adding the
/// header everywhere would quietly move them all onto the replay path.
async fn call_keyed(
    state: &ApiState,
    space: &str,
    bearer: &str,
    name: &str,
    owner: &str,
    extra: Value,
    key: Option<&str>,
) -> (StatusCode, Value) {
    let mut body = json!({ "$space": space, "name": name, "owner": owner });
    for (k, v) in extra.as_object().expect("an object").clone() {
        body[k] = v;
    }
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(format!("/op/{CREATE}"))
        .header("x-ordo-space", space)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {bearer}"));
    if let Some(key) = key {
        req = req.header("idempotency-key", key);
    }
    let req = req.body(Body::from(body.to_string())).unwrap();
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
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

/// The door's own engine, rebuilt from the `members` rows that are
/// actually in the database — which is what a replica does when the mint
/// announces itself, and what a restart does regardless.
/// The door's own engine, rebuilt **the way a deployment rebuilds it**.
///
/// This read the control database alone until 2026-09-25, which was right
/// while unattached tenants lived there. Every tenant now has a database of
/// its own and its founding member row is written to it, so a control-only
/// read finds no membership and the tenant looks unreachable. That is §9.3
/// D-2, the drift the plan calls *"the kind of defect that looks like an
/// authorisation bug for a day"* — `runtime.rs` already closes it by
/// building through `directory::routed_engine`, and this now does the same
/// rather than testing a path production does not take.
async fn engine_now(h: &Harness) -> AccessEngine {
    let credentials = ordo_exec::route::WithOwnCredential::wrapping(
        Arc::new(ordo_exec::route::Installed::new().as_configured("local")),
        ordo_exec::PoolConfig::from_env().password.clone(),
    );
    let template = ordo_exec::PoolConfig {
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    };
    ordo_host::directory::routed_engine(&h.fixture, &h.pool, credentials.as_ref(), &template)
        .await
        .expect("the routed directory")
        .engine
}

// ── the done-when ────────────────────────────────────────────────────────

/// A deployment owner mints a tenant, and everything the tenant needs to
/// exist is there: an id nobody chose, a name that is only a name, a
/// founding owner, and a door that reaches it.
#[tokio::test(flavor = "multi_thread")]
async fn a_minted_tenant_answers_to_its_owner() {
    let h = harness("ordo_tcreate_owner").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "Acme Corp", CUSTOMER).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let row = &body["rows"][0];
    let id = row["space"].as_str().expect("a space id").to_string();
    assert!(id.starts_with("spc_"), "an id the platform minted: {id}");
    assert_eq!(row["name"], "Acme Corp");
    assert_eq!(row["owner"], CUSTOMER);
    assert_eq!(row["reachable"], json!(true), "{body}");

    // The name is recorded, and it is recorded as an attribute of the id.
    let tenant = tenants::lookup(&h.pool, &id)
        .await
        .expect("registry")
        .expect("a row for the tenant that was just created");
    assert_eq!(tenant.name, "Acme Corp");
    assert_eq!(tenant.created_by, DEP_OWNER, "the caller, not the owner");

    // The property the first version of this op got wrong: the door
    // rebuilt from the real rows reaches the tenant **as its owner**, not
    // as some identifier the platform invented.
    let door = engine_now(&h).await;
    assert!(
        door.membership_status(&ISpace(id.clone()), &PrincipalId(CUSTOMER.into()))
            .is_some(),
        "the owner named at creation has no membership in the space created for them"
    );
    assert!(
        door.membership_status(&ISpace(id.clone()), &PrincipalId(DEP_OWNER.into()))
            .is_none(),
        "the operator who minted the tenant became a member of it"
    );
}

/// *Nothing anywhere keys on its name.* Two tenants, one name, two ids,
/// two spaces, and both are reachable by their own owners.
#[tokio::test(flavor = "multi_thread")]
async fn two_tenants_may_share_one_name() {
    let h = harness("ordo_tcreate_samename").await;
    let (s1, b1) = call(&h.state, HOME, &token(DEP_OWNER), "Acme", "ada").await;
    let (s2, b2) = call(&h.state, HOME, &token(DEP_OWNER), "Acme", "grace").await;
    assert_eq!(s1, StatusCode::OK, "{b1}");
    assert_eq!(s2, StatusCode::OK, "{b2}");

    let one = b1["rows"][0]["space"].as_str().unwrap().to_string();
    let two = b2["rows"][0]["space"].as_str().unwrap().to_string();
    assert_ne!(one, two, "one name produced one space");

    let door = engine_now(&h).await;
    for (space, owner) in [(&one, "ada"), (&two, "grace")] {
        assert!(
            door.membership_status(&ISpace(space.clone()), &PrincipalId(owner.into()))
                .is_some(),
            "{owner} cannot reach {space}"
        );
    }
    // And each name still resolves to exactly the id it was recorded
    // under, which is the only direction the registry answers in.
    assert_eq!(
        tenants::lookup(&h.pool, &one).await.unwrap().unwrap().name,
        "Acme"
    );
    assert_eq!(
        tenants::lookup(&h.pool, &two).await.unwrap().unwrap().name,
        "Acme"
    );
}

// ── the gate ─────────────────────────────────────────────────────────────

/// Check 3. A tenant's owner holds `Role::Owner` and passes both the
/// profile and step-up, and must still not be able to mint a tenant into
/// the deployment that hosts them.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_owner_cannot_mint_a_tenant() {
    let h = harness("ordo_tcreate_tenant_owner").await;
    let (status, body) = call(&h.state, TENANT, &token(TENANT_OWNER), "Theirs", "eve").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(code(&body), "not_found", "{body}");
}

/// Check 1. A manager of the deployment's own space administers who may do
/// what inside it. Deciding who the deployment serves is not that.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_manager_cannot_mint_a_tenant() {
    let h = harness("ordo_tcreate_manager").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_MANAGER), "Theirs", "eve").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied", "{body}");
}

/// Check 2. The strongest capability token that can name this op is still
/// refused, because step-up is structurally human-only.
#[tokio::test(flavor = "multi_thread")]
async fn a_capability_token_cannot_mint_a_tenant() {
    let h = harness("ordo_tcreate_capability").await;
    let bearer = capability_token(&h.engine, DEP_OWNER);
    let (status, body) = call(&h.state, HOME, &bearer, "Theirs", "eve").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied", "{body}");
}

// ── what a caller can get wrong ──────────────────────────────────────────

/// A principal the door could never address would be a space whose owner
/// cannot enter it. Refused before anything is written, and named.
#[tokio::test(flavor = "multi_thread")]
async fn an_owner_the_door_cannot_address_is_refused_and_nothing_is_created() {
    let h = harness("ordo_tcreate_badowner").await;
    let before = tenants::all(&h.pool).await.expect("registry").len();
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "Acme", "user:ada").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(code(&body), "tenant_refused", "{body}");
    assert_eq!(
        tenants::all(&h.pool).await.expect("registry").len(),
        before,
        "a refused create left a row behind"
    );
}

/// A blank name is a tenant nobody can pick out of a list, and the id is
/// unreadable by design, so there would be nothing to pick it out by.
#[tokio::test(flavor = "multi_thread")]
async fn a_blank_name_is_refused() {
    let h = harness("ordo_tcreate_blankname").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "   ", CUSTOMER).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(code(&body), "tenant_refused", "{body}");
}

// ── Every tenant gets a database, and the call picks the cluster ───────

/// Force a tenant into `failed`, as a fixture.
///
/// Not `tenants::fail`, which is guarded on `provisioning` so that a late
/// failure cannot un-activate a tenant that is serving. That guard is the
/// reason a healthy tenant can never be reaped, and it means a test that
/// wants a reapable tenant with a real database has to set the state
/// directly.
async fn force_failed(pool: &ordo_exec::Pool, id: &str) {
    pool.rows(&format!(
        "UPDATE \"$spaces\" SET \"state\" = 'failed' WHERE \"$space\" = {}",
        ordo_exec::sql_lit(id)
    ))
    .await
    .expect("forced");
}

/// `std.space.reap` through the door.
async fn reap(state: &ApiState, bearer: &str, tenant: &str) -> (StatusCode, Value) {
    let body = json!({ "$space": HOME, "tenant": tenant });
    let req = Request::builder()
        .method(Method::POST)
        .uri("/op/std.space.reap")
        .header("x-ordo-space", HOME)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Does a database of this name exist on the conformance cluster?
async fn database_exists(pool: &ordo_exec::Pool, name: &str) -> bool {
    let rows = pool
        .rows(&format!(
            "SELECT 1 FROM pg_database WHERE datname = {}",
            ordo_exec::sql_lit(name)
        ))
        .await
        .expect("pg_database");
    !rows.is_empty()
}

/// A target database left behind by a previous run.
fn drop_database(name: &str) {
    let _ = Pg {
        db: "postgres".into(),
        ..Pg::default()
    }
    .exec(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"));
}

/// A per-tenant cluster (level 4), named in full because this deployment was
/// not configured with it. There is no `$db`: the name is minted.
fn on_cluster(host: &str) -> Value {
    let env = ordo_exec::PoolConfig::from_env();
    json!({
        "dbHost": host,
        "dbPort": env.port,
        "dbUser": env.user,
        "dbCredential": "local",
        "dbOwnPerimeter": false,
    })
}

/// **A tenant created with no routing parameters still gets a database of
/// its own**, on the deployment's default tenant cluster (ruled
/// 2026-09-25).
///
/// This is the whole of the ruling in one call: *"1 tenant one database."*
/// The caller asks for nothing, and the tenant is not put in the control
/// database beside every other tenant, which is what happened before.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_created_with_no_cluster_still_gets_its_own_database() {
    let h = harness("ordo_tcreate_default").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "Acme Corp", CUSTOMER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["rows"][0]["space"]
        .as_str()
        .expect("a space id")
        .to_string();

    // The attachment exists, and the database is **not** the control one.
    let row = ordo_exec::spacedb::lookup(&h.pool, &ordo_crypto::SpaceId(id.clone()))
        .await
        .expect("$spacedb")
        .expect("every tenant has a database");
    assert_eq!(
        row.db, id,
        "the database name is minted from the space id, not chosen"
    );
    assert_ne!(
        row.db, "ordo_tcreate_default",
        "the tenant landed in the control database"
    );
    assert_eq!(row.attached_by, ordo_host::provision::SYSTEM_ACTOR);

    // The founding member is in **that** database. This is the half that
    // would look like an authorisation bug: the door reads memberships
    // through the routed directory, and a control-only read finds none.
    let theirs = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: id.clone(),
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("the tenant's own database");
    let members = theirs
        .rows(&format!(
            "SELECT \"id\" FROM \"members\" WHERE \"$space\" = '{id}'"
        ))
        .await
        .expect("members");
    assert_eq!(
        members.len(),
        1,
        "the founding row is not in the tenant's db"
    );

    drop_database(&id);
}

/// **A per-tenant cluster still works** (level 4). The call names the
/// cluster in full, and the tenant's database is minted there, dialled
/// under the credential label the call gave.
#[tokio::test(flavor = "multi_thread")]
async fn a_per_tenant_cluster_still_works() {
    let h = harness("ordo_tcreate_own_cluster").await;
    let env = ordo_exec::PoolConfig::from_env();
    let (status, body) = call_with(
        &h.state,
        HOME,
        &token(DEP_OWNER),
        "Acme",
        CUSTOMER,
        on_cluster(&env.host),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["rows"][0]["space"]
        .as_str()
        .expect("an id")
        .to_string();
    let row = ordo_exec::spacedb::lookup(&h.pool, &ordo_crypto::SpaceId(id.clone()))
        .await
        .expect("$spacedb")
        .expect("the tenant has a database");
    assert_eq!(row.db, id, "the database name is minted");
    assert_eq!(row.credential, "local", "dialled under the call's label");
    assert!(database_exists(&h.pool, &id).await, "no database was made");
    drop_database(&id);
}

/// **With no credential provider, a per-tenant cluster under a label of its
/// own is refused at the call, naming the setting**, and nothing is built.
/// The deployment's own cluster still needs none.
#[tokio::test(flavor = "multi_thread")]
async fn a_per_tenant_cluster_without_a_credential_provider_is_refused_naming_the_setting() {
    let h = harness_without_a_credential_provider("ordo_tcreate_no_provider").await;
    let env = ordo_exec::PoolConfig::from_env();
    let (status, body) = call_with(
        &h.state,
        HOME,
        &token(DEP_OWNER),
        "Acme",
        CUSTOMER,
        on_cluster(&env.host),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(code(&body), "tenant_refused", "{body}");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("ORDO_TENANT_CREDENTIALS") && message.contains("`local`"),
        "the refusal names the label and the setting: {message}"
    );
    assert!(
        tenants::all(&h.pool).await.expect("$spaces").is_empty(),
        "a refused call left a registry row"
    );

    // The default cluster is dialled as `$deployment` and needs no provider.
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "Acme", CUSTOMER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["rows"][0]["space"]
        .as_str()
        .expect("an id")
        .to_string();
    assert!(database_exists(&h.pool, &id).await, "no database was made");
    drop_database(&id);
}

/// **Two tenants get two databases on the one cluster.**
///
/// The cluster is shared and the database is not, which is D-D's
/// *"database/schema not cluster"* and this ruling saying the same thing
/// from two directions. Without this, the test above would pass on a build
/// that gave every tenant the same database.
#[tokio::test(flavor = "multi_thread")]
async fn two_tenants_get_two_databases_on_one_cluster() {
    let h = harness("ordo_tcreate_two").await;
    let mut dbs = Vec::new();
    for owner in ["ada", "grace"] {
        let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "Acme", owner).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let id = body["rows"][0]["space"].as_str().unwrap().to_string();
        let row = ordo_exec::spacedb::lookup(&h.pool, &ordo_crypto::SpaceId(id.clone()))
            .await
            .unwrap()
            .expect("attached");
        dbs.push((id, row));
    }
    assert_ne!(dbs[0].1.db, dbs[1].1.db, "two tenants share one database");
    assert_eq!(
        (dbs[0].1.host.as_str(), dbs[0].1.port),
        (dbs[1].1.host.as_str(), dbs[1].1.port),
        "the cluster is shared, which is what keeps this affordable"
    );
    for (id, _) in dbs {
        drop_database(&id);
    }
}

/// **A half-stated cluster is refused, not completed.**
///
/// The dangerous shape: a caller who names a user and forgets the host
/// asked for another cluster. Completing the address from this deployment's
/// own connection would put the tenant on the default cluster while the
/// caller believed otherwise.
#[tokio::test(flavor = "multi_thread")]
async fn a_half_stated_cluster_is_refused_rather_than_completed() {
    let h = harness("ordo_tcreate_half").await;
    let env = ordo_exec::PoolConfig::from_env();

    for extra in [
        json!({ "dbUser": env.user }),
        json!({ "dbCredential": "local" }),
        json!({ "dbOwnPerimeter": true }),
    ] {
        let (status, body) = call_with(
            &h.state,
            HOME,
            &token(DEP_OWNER),
            "Acme",
            CUSTOMER,
            extra.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{extra}: {body}");
        assert_eq!(code(&body), "tenant_refused", "{body}");
    }

    // And a host with no custody stated. Defaulting it would be the
    // platform choosing a disclosure on the owner's behalf.
    let mut partial = on_cluster(&env.host);
    partial["dbOwnPerimeter"] = Value::Null;
    let (status, body) =
        call_with(&h.state, HOME, &token(DEP_OWNER), "Acme", CUSTOMER, partial).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(code(&body), "tenant_refused", "{body}");
}

/// **A host off the allowlist is refused with one reason** (R-X3).
///
/// The message names no detail about the allowlist, on
/// `ordo_egress::refusal`'s argument: a distinguishable refusal is an
/// oracle a caller maps one attempt at a time.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_off_the_allowlist_is_refused_with_one_reason() {
    let h = harness("ordo_tcreate_allow").await;
    let (status, body) = call_with(
        &h.state,
        HOME,
        &token(DEP_OWNER),
        "Acme",
        CUSTOMER,
        on_cluster("db.somewhere.else"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(code(&body), "tenant_refused", "{body}");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        !message.contains("ORDO_PG_ATTACH_HOSTS"),
        "the refusal names the control that refused it: {message}"
    );
    // **And it left no tenant behind** (11-15, `bootstrap.md` §1.2). A
    // mistyped host is a mistake, not a stuck provision, and until phase 0
    // was a phase of its own this refusal came *after* `reserve` and put a
    // `failed` row in `tenants::troubled` — the deployment's one detection
    // query for provisions that need a human.
    assert!(
        tenants::all(&h.pool).await.expect("$spaces").is_empty(),
        "a refused parameter left a registry row"
    );
}

/// **The deployment's own cluster is allowed without being named**
/// (D-S6: *"if i defined it on my platform it is allowed"*).
///
/// The zero-configuration case, and the one that would break every
/// deployment if it were wrong: with `ORDO_PG_ATTACH_HOSTS` unset the
/// allowlist denies everything, so a deployment would refuse itself its own
/// default tenant cluster and no tenant could ever be created.
#[tokio::test(flavor = "multi_thread")]
async fn the_deployments_own_cluster_needs_no_allowlist_entry() {
    let env = ordo_exec::PoolConfig::from_env();
    assert!(
        !ordo_host::provision::HostAllowlist::of("").allows(&env.host),
        "the premise: unset denies everything"
    );
    let widened = ordo_host::provision::HostAllowlist::of("").allowing(&env.host);
    assert!(
        widened.allows(&env.host),
        "a host this deployment configured is allowed by definition"
    );
    // And it still refuses everything else, so this is not a way to open
    // the list.
    assert!(!widened.allows("db.somewhere.else"));
}

/// **An unreachable cluster is a 503 whose message names no topology.**
///
/// `docs/api-engine-design.md` documents `space_unavailable` with this
/// invariant: a dial error names a host, a port and often a user, which is
/// deployment topology a caller has no business reading.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_cluster_says_so_without_naming_the_topology() {
    let h = harness("ordo_tcreate_unreach").await;
    let env = ordo_exec::PoolConfig::from_env();
    let mut spec = on_cluster(&env.host);
    spec["dbPort"] = json!(1);
    spec["dbUser"] = json!("nobody");
    let (status, body) = call_with(&h.state, HOME, &token(DEP_OWNER), "Acme", CUSTOMER, spec).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a dependency being down is a retry, not a 400 or a 500: {body}"
    );
    assert_eq!(code(&body), "space_unavailable", "{body}");

    let message = body["error"]["message"].as_str().unwrap_or_default();
    for leak in [
        "error connecting",
        "os error",
        "the space's database",
        env.host.as_str(),
        "nobody",
    ] {
        assert!(
            !message.to_lowercase().contains(&leak.to_lowercase()),
            "the refusal carries `{leak}`, which is topology rather than an answer: {message}"
        );
    }

    // **And it left no tenant behind** (11-15). This is the refusal that
    // used to cost the most: the dial happened inside `create_database`,
    // which is phase 3, so a wrong port produced a registry row, a custody
    // key, and a `failed` tenant an operator then had to reap. Phase 0
    // dials first, so a cluster that does not answer is an answer.
    assert!(
        tenants::all(&h.pool).await.expect("$spaces").is_empty(),
        "an unreachable cluster left a registry row"
    );
}

/// **`dryRun` runs phase 0 and reserves nothing** (`bootstrap.md` §2 item
/// 45: *"It writes nothing and reserves nothing, so two dry runs of the
/// same slug both succeed."*).
///
/// The plan is asserted field by field rather than by shape, because a
/// plan that is present and empty is worse than no plan: an operator reads
/// it and concludes the deployment resolved nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_returns_the_plan_and_creates_nothing() {
    let h = harness("ordo_tcreate_dryrun").await;
    let (status, body) = call_with(
        &h.state,
        HOME,
        &token(DEP_OWNER),
        "Acme",
        CUSTOMER,
        json!({ "dryRun": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];

    // No id, because nothing was reserved. A minted id here would read as
    // a handle to something that does not exist.
    assert!(row["space"].is_null(), "a dry run minted an id: {body}");
    assert!(
        row["reachable"].is_null(),
        "`false` would say a tenant exists and cannot be reached: {body}"
    );

    let plan = &row["plan"];
    let db = plan["database"].as_str().unwrap_or_default();
    assert!(
        db.starts_with("spc_"),
        "the plan does not name the database it would make: {plan}"
    );
    assert!(
        plan["target"]
            .as_str()
            .unwrap_or_default()
            .contains(&format!("/{db}")),
        "the plan's target does not name its own database: {plan}"
    );
    assert!(
        !plan["version"].as_str().unwrap_or_default().is_empty(),
        "the plan does not say which schema the tenant would receive: {plan}"
    );
    assert_eq!(
        plan["databaseStanding"],
        json!(false),
        "a minted database name cannot already exist: {plan}"
    );
    assert_eq!(plan["occupancy"], json!(0), "{plan}");

    // Nothing was reserved and nothing was made.
    assert!(
        tenants::all(&h.pool).await.expect("$spaces").is_empty(),
        "a dry run reserved a tenant"
    );
    assert!(
        !database_exists(&h.pool, db).await,
        "a dry run created `{db}`"
    );

    // **Twice**, which is the clause item 45 states outright and the one
    // that fails if a dry run takes a slug, a lock or an id and keeps it.
    // A second dry run also proves the routing lock was released: phase 0
    // holds it, and a `Preflight` dropped without committing is the whole
    // of abandoning a plan.
    let (status, body) = call_with(
        &h.state,
        HOME,
        &token(DEP_OWNER),
        "Acme",
        CUSTOMER,
        json!({ "dryRun": true }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a second dry run was refused: {body}"
    );

    // And the real call still works afterwards, which is the property a
    // leaked lock would break and a leaked reservation would not.
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "Acme", CUSTOMER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["rows"][0]["space"]
        .as_str()
        .expect("a space")
        .to_string();
    assert!(
        body["rows"][0]["plan"].is_null(),
        "a real create rendered a plan: {body}"
    );
    drop_database(&id);
}

/// **A dry run cannot hand its answer to a real call under the same
/// `Idempotency-Key`.**
///
/// The one interaction between item 44 and item 45 that could corrupt an
/// answer, and it needs pinning rather than reasoning about: if the replay
/// cache keyed on the header alone, an operator who dry-ran with a key and
/// then re-sent with `dryRun: false` and the same key would be handed the
/// plan back — `space: null`, status 200 — and would believe a tenant was
/// created that does not exist.
///
/// It does not, because the cache binds the key to a **fingerprint of the
/// payload** and `dryRun: true` is a different payload. The answer is
/// therefore a refusal rather than a wrong success, which is the right
/// direction, and `bootstrap.md` §6.1's worked example already assumes it:
/// the dry run there carries no key, and the key appears on the call that
/// runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_cannot_answer_for_the_create_that_follows_it() {
    let h = harness("ordo_tcreate_dryrun_key").await;
    const KEY: &str = "one-key-two-payloads";

    let (status, body) = call_keyed(
        &h.state,
        HOME,
        &token(DEP_OWNER),
        "Acme",
        CUSTOMER,
        json!({ "dryRun": true }),
        Some(KEY),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["rows"][0]["space"].is_null(), "{body}");

    // The same key, the real payload. It must not be the dry run's answer.
    let (status, body) = call_keyed(
        &h.state,
        HOME,
        &token(DEP_OWNER),
        "Acme",
        CUSTOMER,
        json!({}),
        Some(KEY),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "the create was answered by the dry run it followed: {body}"
    );
    assert_eq!(code(&body), "idempotency_mismatch", "{body}");

    // And nothing was created under either call, so the refusal is a
    // refusal rather than a create whose answer went missing.
    assert!(
        tenants::all(&h.pool).await.expect("$spaces").is_empty(),
        "a tenant exists after a dry run and a refused replay"
    );

    // **The positive control, so the assertion above is not passing because
    // the replay cache is doing nothing.** The *same* payload under the
    // same key is a hit and comes back 200 with the same answer. If the
    // cache were inert, the refusal above would have been a success and
    // this would be a second live run — either way the two halves cannot
    // both hold unless the key is bound to the payload.
    let (status, again) = call_keyed(
        &h.state,
        HOME,
        &token(DEP_OWNER),
        "Acme",
        CUSTOMER,
        json!({ "dryRun": true }),
        Some(KEY),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert!(again["rows"][0]["space"].is_null(), "{again}");
}

/// **A dry run refuses what a create would refuse**, which is the only
/// thing that makes it worth calling first.
///
/// A `dryRun` that accepted a host the real call rejects would be a worse
/// surface than no dry run at all: it would move the discovery of the
/// mistake to the call that has consequences.
#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_refuses_a_host_a_create_would_refuse() {
    let h = harness("ordo_tcreate_dryrun_refuse").await;
    let mut spec = on_cluster("db.somewhere.else");
    spec["dryRun"] = json!(true);
    let (status, body) = call_with(&h.state, HOME, &token(DEP_OWNER), "Acme", CUSTOMER, spec).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(code(&body), "tenant_refused", "{body}");
    assert!(
        tenants::all(&h.pool).await.expect("$spaces").is_empty(),
        "a refused dry run left a registry row"
    );
}

/// **A deployment with no routing at all refuses to create a tenant**,
/// rather than putting it in the control database.
///
/// Unreachable from `runtime.rs`, which always builds a default cluster
/// from the deployment's own connection. Asserted because the alternative
/// failure is silent: a tenant in the control database whose disclosure
/// says otherwise is the mistake nobody finds until it is already wrong.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_that_cannot_make_databases_refuses_to_make_tenants() {
    let h = unrouted_harness("ordo_tcreate_unrouted").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "Acme", CUSTOMER).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(code(&body), "space_unavailable", "{body}");

    let spaces = h
        .pool
        .rows("SELECT \"$space\" FROM \"$spaces\"")
        .await
        .expect("$spaces");
    assert!(spaces.is_empty(), "a tenant was created anyway: {spaces:?}");
}

// ── 11-15: the provisioning lifecycle ──────────────────────────────────

/// **A created tenant is `active`, and its registry row says where it
/// lives** (`bootstrap.md` §4.1).
#[tokio::test(flavor = "multi_thread")]
async fn a_created_tenant_is_active_and_its_row_names_its_database() {
    let h = harness("ordo_tcreate_lifecycle").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "Acme", CUSTOMER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["rows"][0]["space"].as_str().unwrap().to_string();

    let t = tenants::lookup(&h.pool, &id)
        .await
        .expect("registry")
        .expect("a row");
    assert_eq!(t.state, tenants::State::Active);
    assert_eq!(t.phase, tenants::Phase::Founded);
    assert!(t.target.ends_with(&format!("/{id}")), "{:?}", t.target);
    assert!(t.state_at > 0, "the activation is timestamped");
    assert!(
        tenants::troubled(&h.pool)
            .await
            .expect("troubled")
            .is_empty(),
        "a healthy deployment has nothing in the troubled pane"
    );
    drop_database(&id);
}

/// **A provision that fails leaves a `failed` row naming what it was
/// building**, rather than leaving nothing.
///
/// This is the whole point of 11-15. Before it, the registry row was
/// written **last**, so a failure part way left custody keys and possibly
/// a database with nothing naming them: §4.4's *"every orphan is reachable
/// from one table"* was false. The row is deliberately not deleted on
/// failure — deleting it would be the leak arriving through the tidy-up
/// path.
/// **The mechanism changed with phase 0 and the reason is the increment.**
/// This used to fail the provision by naming an unreachable port, which
/// worked because the dial happened inside `create_database` — phase 3,
/// after the row. Phase 0 now dials first, so that call is refused with no
/// row at all, which is the fix and is asserted in
/// `an_unreachable_cluster_says_so_without_naming_the_topology`. The
/// failure that remains after the row is an external step that cannot be
/// transactional, and the first of those in `bootstrap.md` §4.3 is the
/// KMS. So this now refuses custody, which is both a realer fault and the
/// one the compensation table is actually about.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_provision_leaves_a_row_that_names_what_it_was_building() {
    let h = harness_whose_keys_refuse("ordo_tcreate_failed").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "Acme", CUSTOMER).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a key provider that will not answer is a dependency being down: {body}"
    );

    // **And the answer says a tenant was left**, because one was. Every
    // refusal that reaches a caller before the row says "nothing was
    // created"; this one must not, or an operator has no reason to look
    // for the row they now have to reap.
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("std.space.reap"),
        "the answer does not say what to do about the tenant it left: {message}"
    );

    let troubled = tenants::troubled(&h.pool).await.expect("troubled");
    assert_eq!(troubled.len(), 1, "{troubled:?}");
    let t = &troubled[0];
    assert_eq!(t.state, tenants::State::Failed);
    // The target was recorded **before** the dial, which is what makes a
    // database that may have been created findable.
    assert!(t.target.contains(&t.id), "{:?}", t.target);
    assert!(!t.note.is_empty(), "the failure says why");
}

/// **Reaping releases the attachment, the database and the keys, and lands
/// the tenant in `reaped`** (§4.5).
#[tokio::test(flavor = "multi_thread")]
async fn reaping_a_failed_tenant_releases_its_database_and_row() {
    let h = harness("ordo_tcreate_reap").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "Acme", CUSTOMER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["rows"][0]["space"].as_str().unwrap().to_string();

    // It is active, so it must be refused: cleaning up a failed provision
    // and destroying a customer's world are different buttons (§4.5).
    let (status, body) = reap(&h.state, &token(DEP_OWNER), &id).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(code(&body), "tenant_refused", "{body}");
    assert!(
        database_exists(&h.pool, &id).await,
        "a refused reap dropped the database anyway"
    );

    // Put it in a state a reap is for.
    force_failed(&h.pool, &id).await;

    let (status, body) = reap(&h.state, &token(DEP_OWNER), &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["outcome"], "reaped", "{body}");

    assert!(
        !database_exists(&h.pool, &id).await,
        "the tenant's database is still there"
    );
    assert!(
        ordo_exec::spacedb::lookup(&h.pool, &ordo_crypto::SpaceId(id.clone()))
            .await
            .unwrap()
            .is_none(),
        "the attachment is still there"
    );
    let t = tenants::lookup(&h.pool, &id).await.unwrap().expect("row");
    assert_eq!(t.state, tenants::State::Reaped);

    // Idempotent: reaping again says so rather than failing.
    let (status, body) = reap(&h.state, &token(DEP_OWNER), &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["outcome"], "already reaped", "{body}");
}

/// **A tenant's own perimeter is reported and left standing** (§4.3): *"the
/// platform never issues an unattended `DROP SCHEMA` against a customer's
/// database."*
#[tokio::test(flavor = "multi_thread")]
async fn reaping_never_drops_a_database_the_platform_did_not_create() {
    let h = harness("ordo_tcreate_reap_byo").await;
    let env = ordo_exec::PoolConfig::from_env();
    let mut spec = on_cluster(&env.host);
    spec["dbOwnPerimeter"] = json!(true);
    let (status, body) = call_with(&h.state, HOME, &token(DEP_OWNER), "Acme", CUSTOMER, spec).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["rows"][0]["space"].as_str().unwrap().to_string();

    force_failed(&h.pool, &id).await;
    let (status, body) = reap(&h.state, &token(DEP_OWNER), &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let steps = body["rows"][0]["released"].as_array().expect("steps");
    let db = steps
        .iter()
        .find(|s| s["step"] == "database")
        .expect("a database step");
    assert_eq!(db["outcome"], "left standing", "{db}");
    assert!(
        database_exists(&h.pool, &id).await,
        "the platform dropped a database it did not create"
    );
    drop_database(&id);
}

/// **A boot does not activate a tenant that is still being built.**
///
/// `ensure_table` backfills rows that predate the lifecycle columns:
/// without it, every `$spaces` row created before 11-15 would read as
/// `provisioning` and every existing tenant would quietly go out of
/// service. It tells an old row from a new one by `state_at = 0`, which
/// works only because `reserve` stamps that field.
///
/// It did not, at first. A genuinely half-built tenant also had zero
/// there, so the next boot would have declared it `active` — a tenant
/// announced as ready with no database, produced by the code written to
/// smooth an upgrade. This is the test that would have caught it.
#[tokio::test(flavor = "multi_thread")]
async fn a_boot_does_not_activate_a_tenant_that_is_still_being_built() {
    let h = harness("ordo_tcreate_backfill").await;

    tenants::reserve(&h.pool, "spc_halfbuilt", "Half Built", DEP_OWNER, NOW, None)
        .await
        .expect("reserved");

    // What every boot runs.
    tenants::ensure_table(&h.pool).await.expect("ensure");

    let t = tenants::lookup(&h.pool, "spc_halfbuilt")
        .await
        .expect("registry")
        .expect("a row");
    assert_eq!(
        t.state,
        tenants::State::Provisioning,
        "a boot activated a tenant that was still being built"
    );

    // And the backfill it exists for still works: a row shaped like one
    // written before the lifecycle columns reads as active.
    h.pool
        .rows(&format!(
            "UPDATE \"$spaces\" SET \"state\" = 'provisioning', \"state_at\" = 0 \
             WHERE \"$space\" = {}",
            ordo_exec::sql_lit("spc_halfbuilt")
        ))
        .await
        .expect("aged");
    tenants::ensure_table(&h.pool).await.expect("ensure");
    let t = tenants::lookup(&h.pool, "spc_halfbuilt")
        .await
        .unwrap()
        .expect("a row");
    assert_eq!(
        t.state,
        tenants::State::Active,
        "a tenant that predates the lifecycle was taken out of service"
    );
}

/// **A provision that stops part way is timed out and becomes visible**
/// (`bootstrap.md` §4.1's `bootstrapTimeout` reconciler).
///
/// Without this a stuck tenant sits in `provisioning` until somebody goes
/// looking, which is the difference between a failure that is findable and
/// one that is found.
#[tokio::test(flavor = "multi_thread")]
async fn a_stuck_provision_is_timed_out_and_lands_in_the_troubled_list() {
    let h = harness("ordo_tcreate_reconcile").await;
    tenants::reserve(&h.pool, "spc_stuck", "Stuck", DEP_OWNER, NOW, None)
        .await
        .expect("reserved");
    tenants::at_phase(
        &h.pool,
        "spc_stuck",
        tenants::Phase::Targeted,
        "db.internal:5432/spc_stuck",
        NOW,
    )
    .await
    .expect("targeted");

    // Not yet: a provision inside its window is a provision still running,
    // and a reconciler that fired early would race the call.
    let n = tenants::time_out_stale(&h.pool, 900, NOW + 60)
        .await
        .expect("sweep");
    assert_eq!(n, 0, "a provision inside its window was timed out");
    assert_eq!(
        tenants::lookup(&h.pool, "spc_stuck")
            .await
            .unwrap()
            .unwrap()
            .state,
        tenants::State::Provisioning
    );

    // Past the timeout.
    let n = tenants::time_out_stale(&h.pool, 900, NOW + 901)
        .await
        .expect("sweep");
    assert_eq!(n, 1, "a stuck provision was not timed out");

    let t = tenants::lookup(&h.pool, "spc_stuck")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, tenants::State::Failed);
    // The note names the phase and the target, which is what tells an
    // operator how much was built and where to look.
    assert!(t.note.contains("targeted"), "{}", t.note);
    assert!(t.note.contains("spc_stuck"), "{}", t.note);
    assert!(
        tenants::troubled(&h.pool)
            .await
            .unwrap()
            .iter()
            .any(|x| x.id == "spc_stuck"),
        "the timed-out tenant is not in the troubled list"
    );

    // And it is idempotent: a second sweep finds nothing to do, because
    // `fail` is guarded on `provisioning`.
    let n = tenants::time_out_stale(&h.pool, 900, NOW + 2000)
        .await
        .expect("sweep");
    assert_eq!(n, 0, "the sweep re-failed a tenant it had already failed");
}

/// **The reconciler never touches an active tenant**, however old it is.
///
/// `state_at` is set by activation too, so a deployment quiet for a week
/// has active tenants whose timestamps are ancient. A sweep that looked at
/// age alone would mark every one of them failed.
#[tokio::test(flavor = "multi_thread")]
async fn the_reconciler_ignores_tenants_that_are_active_however_old() {
    let h = harness("ordo_tcreate_reconcile_active").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "Acme", CUSTOMER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["rows"][0]["space"].as_str().unwrap().to_string();

    // A year after **the row's own timestamp**, not after the test's `NOW`
    // constant. `TenantOps::create` stamps `state_at` from the real wall
    // clock rather than the harness's injected one, so arithmetic from
    // `NOW` lands *before* the tenant exists and the sweep skips it for
    // the wrong reason. The first version of this test did exactly that
    // and passed with both of the reconciler's guards deliberately
    // removed, which is how the flaw was found.
    let stamped = tenants::lookup(&h.pool, &id)
        .await
        .unwrap()
        .expect("a row")
        .state_at;
    let n = tenants::time_out_stale(&h.pool, 900, stamped + 31_536_000)
        .await
        .expect("sweep");
    assert_eq!(n, 0, "the reconciler timed out a live tenant");
    assert_eq!(
        tenants::lookup(&h.pool, &id).await.unwrap().unwrap().state,
        tenants::State::Active
    );
    drop_database(&id);
}

// ── test doubles ─────────────────────────────────────────────────────────

/// A provider that will not mint custody for a new space and forwards
/// everything else — an unhealthy KMS, which is `bootstrap.md` §4.3's
/// first non-transactional step. See `harness_whose_keys_refuse`.
/// A provider whose custody cannot be **opened**: `blind_index` fails, which
/// is the question `ordo_crypto::probe` asks and therefore the question
/// preflight asks. Everything else forwards, so nothing else explains the
/// refusal.
struct UnreadableCustody(Arc<dyn ordo_crypto::KeyProvider>);

impl ordo_crypto::KeyProvider for UnreadableCustody {
    fn blind_index(
        &self,
        _s: &ordo_crypto::SpaceId,
        _e: &str,
        _f: &str,
        _v: ordo_crypto::BlindValue<'_>,
    ) -> Result<[u8; 32], ordo_crypto::KeyError> {
        Err(ordo_crypto::KeyError::NotLeased)
    }
    fn create_space(&self, s: &ordo_crypto::SpaceId) -> Result<(), ordo_crypto::KeyError> {
        self.0.create_space(s)
    }
    fn release(
        &self,
        s: &ordo_crypto::SpaceId,
        p: ordo_crypto::Purpose,
        c: &str,
    ) -> Result<ordo_crypto::KeyLease, ordo_crypto::KeyError> {
        self.0.release(s, p, c)
    }
    fn seal_at(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: ordo_crypto::Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.0.seal_at(l, e, f, a, v)
    }
    fn open_at(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: ordo_crypto::Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.0.open_at(l, e, f, a, v)
    }
    #[allow(clippy::too_many_arguments)]
    fn seal_chunk(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: ordo_crypto::Anchor<'_>,
        ep: ordo_crypto::Epoch,
        b: &ordo_crypto::NonceBase,
        c: u32,
        last: bool,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.0.seal_chunk(l, e, f, a, ep, b, c, last, v)
    }
    #[allow(clippy::too_many_arguments)]
    fn open_chunk(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: ordo_crypto::Anchor<'_>,
        ep: ordo_crypto::Epoch,
        b: &ordo_crypto::NonceBase,
        c: u32,
        last: bool,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.0.open_chunk(l, e, f, a, ep, b, c, last, v)
    }
    fn current_epoch(
        &self,
        s: &ordo_crypto::SpaceId,
    ) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
        self.0.current_epoch(s)
    }
    fn rotate(
        &self,
        s: &ordo_crypto::SpaceId,
        r: &str,
    ) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
        self.0.rotate(s, r)
    }
    fn destroy_epoch(
        &self,
        s: &ordo_crypto::SpaceId,
        e: ordo_crypto::Epoch,
    ) -> Result<(), ordo_crypto::KeyError> {
        self.0.destroy_epoch(s, e)
    }
    fn destroy_space(&self, s: &ordo_crypto::SpaceId) -> Result<(), ordo_crypto::KeyError> {
        self.0.destroy_space(s)
    }
}

struct RefusesNewSpaces(Arc<dyn ordo_crypto::KeyProvider>);

impl ordo_crypto::KeyProvider for RefusesNewSpaces {
    fn create_space(&self, _s: &ordo_crypto::SpaceId) -> Result<(), ordo_crypto::KeyError> {
        Err(ordo_crypto::KeyError::NotLeased)
    }
    fn release(
        &self,
        s: &ordo_crypto::SpaceId,
        p: ordo_crypto::Purpose,
        c: &str,
    ) -> Result<ordo_crypto::KeyLease, ordo_crypto::KeyError> {
        self.0.release(s, p, c)
    }
    fn seal_at(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: ordo_crypto::Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.0.seal_at(l, e, f, a, v)
    }
    fn open_at(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: ordo_crypto::Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.0.open_at(l, e, f, a, v)
    }
    #[allow(clippy::too_many_arguments)]
    fn seal_chunk(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: ordo_crypto::Anchor<'_>,
        ep: ordo_crypto::Epoch,
        b: &ordo_crypto::NonceBase,
        c: u32,
        last: bool,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.0.seal_chunk(l, e, f, a, ep, b, c, last, v)
    }
    #[allow(clippy::too_many_arguments)]
    fn open_chunk(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: ordo_crypto::Anchor<'_>,
        ep: ordo_crypto::Epoch,
        b: &ordo_crypto::NonceBase,
        c: u32,
        last: bool,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.0.open_chunk(l, e, f, a, ep, b, c, last, v)
    }
    fn blind_index(
        &self,
        s: &ordo_crypto::SpaceId,
        e: &str,
        f: &str,
        v: ordo_crypto::BlindValue<'_>,
    ) -> Result<[u8; 32], ordo_crypto::KeyError> {
        self.0.blind_index(s, e, f, v)
    }
    fn current_epoch(
        &self,
        s: &ordo_crypto::SpaceId,
    ) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
        self.0.current_epoch(s)
    }
    fn rotate(
        &self,
        s: &ordo_crypto::SpaceId,
        r: &str,
    ) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
        self.0.rotate(s, r)
    }
    fn destroy_epoch(
        &self,
        s: &ordo_crypto::SpaceId,
        e: ordo_crypto::Epoch,
    ) -> Result<(), ordo_crypto::KeyError> {
        self.0.destroy_epoch(s, e)
    }
    fn destroy_space(&self, s: &ordo_crypto::SpaceId) -> Result<(), ordo_crypto::KeyError> {
        self.0.destroy_space(s)
    }
}

// ── bootstrap.md §2 item 44: the header is required here and nowhere else ──

/// **A create with no `Idempotency-Key` is refused, and nothing is built.**
///
/// The header is optional on every other write in the platform, and the cost
/// of omitting it there is bounded: a retry repeats an insert, and the row's
/// own key, a `rev` CAS or a `converge` mode catches it. That bound does not
/// hold here, because this op's effect is **outside the database it writes
/// to** — a retry after a network failure mints a second KMS key and a second
/// database, answers with the second tenant's id, and leaves the first for
/// somebody to find by reading the cluster. Both rows are `active` and
/// correct; nothing reconciles them.
///
/// So the refusal has to come **before** the executor, which is what this
/// pins: the answer is a 400 and `$spaces` is empty.
#[tokio::test(flavor = "multi_thread")]
async fn a_create_without_an_idempotency_key_is_refused_before_anything_is_built() {
    let h = harness("ordo_tcreate_no_key").await;
    let (status, body) = call_keyed(
        &h.state,
        HOME,
        &token(DEP_OWNER),
        "Acme",
        CUSTOMER,
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(code(&body), "missing_param", "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Idempotency-Key"),
        "the refusal does not name the header the caller must send: {body}"
    );
    assert!(
        tenants::all(&h.pool).await.expect("$spaces").is_empty(),
        "a keyless create built a tenant anyway"
    );
}

/// **And the requirement is this op's, not every write's.**
///
/// The mechanism is a flag on the spec, so it would be easy to make it
/// deployment-wide by accident — and that would be a breaking change to every
/// client for a risk the row-level mechanisms already cover. `std.space.reap`
/// is the nearest neighbour: a platform write, on the same router, in the
/// same module, and it must still answer without the header.
#[tokio::test(flavor = "multi_thread")]
async fn the_requirement_is_this_ops_and_not_every_writes() {
    // The spec is the authority, so read it rather than inferring from a call.
    let specs = platform_ops::specs();
    let create = specs
        .iter()
        .find(|s| s.name == CREATE)
        .expect("std.space.create is registered");
    assert!(
        create.requires_idempotency_key,
        "item 44 is not in force on the op it is about"
    );
    let others: Vec<&str> = specs
        .iter()
        .filter(|s| s.requires_idempotency_key && s.name != CREATE)
        .map(|s| s.name.as_str())
        .collect();
    assert!(
        others.is_empty(),
        "the requirement spread to ops item 44 does not cover: {others:?}"
    );

    // And behaviourally, through the door: a keyless reap is not refused for
    // want of a header. It is refused for being about a tenant that does not
    // exist, which is a different answer and the one that proves the header
    // was never the question.
    let h = harness("ordo_tcreate_key_scope").await;
    let (status, body) = reap(&h.state, &token(DEP_OWNER), "spc_nothing").await;
    assert_ne!(
        code(&body),
        "missing_param",
        "a write that does not require the header was refused for want of one: {status} {body}"
    );
}

// ── bootstrap.md §2 item 22: an unhealthy provider is refused in preflight ──

/// **A key provider that cannot open custody is refused before the registry
/// row exists** (`bootstrap.md` §2 item 22).
///
/// Phase 2 mints the tenant's SMK and is the first step that cannot be rolled
/// back: a KMS is an external system with no part in a Postgres transaction,
/// so its failure leaves a `failed` row whose compensation is a *scheduled
/// destroy* of a key that may or may not exist. Item 22 calls the check that
/// avoids this *"the check that keeps phase 2's non-transactionality from
/// mattering in the common case"*, and the common cases are a provider that is
/// down and a process holding the wrong master.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_key_provider_is_refused_before_a_row_exists() {
    let h = harness_whose_custody_is_unreadable("ordo_tcreate_custody_probe").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "Acme", CUSTOMER).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a provider that cannot answer is a dependency being down: {body}"
    );

    // **And nothing was reserved**, which is the whole of item 22. A refusal
    // that left a row would have made an operator reap a tenant that was
    // never built, and put it in the one list §4.1 keeps for provisions that
    // genuinely need a human.
    assert!(
        tenants::all(&h.pool).await.expect("$spaces").is_empty(),
        "an unhealthy provider left a registry row behind"
    );

    // The message says a dependency is down rather than naming the KMS, the
    // space, or the question asked: a provider's reachability is the
    // deployment's topology, and `/health/deep` is where an operator reads it.
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        !message.contains("$custody") && !message.contains("blind"),
        "the refusal names the probe rather than the condition: {message}"
    );
}

/// **And the probe does not swallow the failure it cannot predict.**
///
/// A provider can be readable and still refuse to *create* — a key ring with
/// no write grant, a quota, a policy. Item 22 never claimed to catch that, and
/// a probe that appeared to would be worse than none: it would turn "the
/// residue is possible" into "the residue is impossible" while leaving the
/// residue possible. So this pins that the phase-2 path is still reachable,
/// which is what `a_failed_provision_leaves_a_row_that_names_what_it_was_building`
/// exercises with a provider whose `blind_index` forwards and whose
/// `create_space` refuses.
#[tokio::test(flavor = "multi_thread")]
async fn a_readable_provider_that_will_not_create_still_fails_in_phase_two() {
    let h = harness_whose_keys_refuse("ordo_tcreate_custody_probe_gap").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "Acme", CUSTOMER).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");

    // A row, and a `failed` one: preflight passed because custody *was*
    // readable, so this is the residue item 22 bounds rather than removes.
    let troubled = tenants::troubled(&h.pool).await.expect("troubled");
    assert_eq!(
        troubled.len(),
        1,
        "the phase-2 failure stopped leaving a row, so the probe is answering \
         a question it cannot answer: {troubled:?}"
    );
    assert_eq!(troubled[0].state, tenants::State::Failed);
}
