//! Reported fixes tenant-access-lags-after-join and
//! pool-exhaustion-reads-as-denied: when the door cannot read a tenant's
//! members, it answers a verified caller `503 server_busy` with
//! `Retry-After`, never `403 access_denied`.
//!
//! Before the fix, three failures each became a denial:
//!
//! - a rebuild after a membership write that could not run (the control
//!   pool full): the door kept the list from before the write, and the
//!   person just admitted was denied until some later rebuild;
//! - a read of the tenant's database that failed with no good list to keep
//!   (a new tenant, a door just booted);
//! - a database server at its connection limit (SQLSTATE 53300), which was
//!   read as a lasting fault, so even the last good list was dropped.
//!
//! The space is attached to its own database, so a read of it is a real
//! read of real member rows, through a role whose connection limit the test
//! sets to 0 to make the database full for the door alone.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_exec::spacedb::{self, DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::member_ops::MemberOps;
use ordo_host::platform_ops;
use ordo_host::runtime::SwappableAccess;
use ordo_identity::principal::{PrincipalId, Timestamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::Value;
use tower::util::ServiceExt;

const SECRET: [u8; 32] = *b"door-behind-test-secret-32bytes!";
/// A viewer in the conformance seed; principal and member id are the same.
const VIEWER: &str = "mem_4";

struct Harness {
    access: Arc<SwappableAccess>,
    state: ApiState,
    /// The door's control pool. Size 1, so holding one connection fills it.
    control: Pool,
    /// A second way in, for writes the door's pool cannot make while full.
    writer: Pool,
    /// The role the door reads the tenant's database as, one per test.
    reader: String,
    deployment: Arc<ordo_host::deploy::Deployment>,
    metrics: Arc<ordo_host::metrics::Metrics>,
    delegations: Arc<ordo_host::delegation::PgDelegationRegistry>,
    credentials: Arc<dyn ordo_exec::route::DbCredentials>,
    space: String,
}

/// The reader role, able to read everything and limited to `limit`
/// connections (`-1` is no limit, `0` is a full database).
async fn reader_limit(h: &Harness, limit: i32) {
    h.writer
        .rows(&format!("ALTER ROLE {} CONNECTION LIMIT {limit}", h.reader))
        .await
        .expect("limit set");
}

async fn harness(db: &str) -> Harness {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");
    let reader = format!("{db}_reader");
    let template = PoolConfig {
        db: db.into(),
        size: 1,
        acquire_timeout: Duration::from_millis(300),
        ..PoolConfig::from_env()
    };
    let control = Pool::connect(template.clone()).await.expect("control pool");
    let writer = Pool::connect(PoolConfig {
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..template.clone()
    })
    .await
    .expect("writer pool");

    // The reader: a plain role, so its connection limit holds.
    let password = std::env::var("ORDO_PG_PASSWORD")
        .ok()
        .filter(|p| !p.is_empty());
    let exists = writer
        .rows(&format!(
            "SELECT 1 FROM pg_roles WHERE rolname = '{reader}'"
        ))
        .await
        .expect("roles");
    let with = match &password {
        Some(p) => format!("LOGIN PASSWORD '{}'", p.replace('\'', "''")),
        None => "LOGIN".to_string(),
    };
    let verb = if exists.is_empty() { "CREATE" } else { "ALTER" };
    writer
        .rows(&format!(
            "{verb} ROLE {reader} {with} NOSUPERUSER CONNECTION LIMIT -1"
        ))
        .await
        .expect("reader role");
    writer
        .rows(&format!("GRANT pg_read_all_data TO {reader}"))
        .await
        .expect("reader reads");

    spacedb::ensure_table(&writer).await.expect("table");
    writer.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    let space = ordo_conformance::fixture_space();
    spacedb::attach(
        &writer,
        &space,
        &DbAttachment {
            host: template.host.clone(),
            port: template.port,
            user: reader.clone(),
            db: db.into(),
            credential: "local".into(),
            custody: DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: "test:door-behind".into(),
        },
    )
    .await
    .expect("attached");

    let grants = Arc::new(ordo_host::grants::PgGrantRegistry::new(writer.clone()));
    grants.ensure_table().await.expect("grants");
    let delegations = Arc::new(ordo_host::delegation::PgDelegationRegistry::new(
        writer.clone(),
    ));
    delegations.ensure_table().await.expect("delegations");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let credentials: Arc<dyn ordo_exec::route::DbCredentials> =
        Arc::new(ordo_exec::route::Installed::new().as_configured("local"));

    // The boot's door, built while the database answers.
    let directory =
        ordo_host::directory::routed_engine(&f, &control, credentials.as_ref(), &template)
            .await
            .expect("the boot's directory");
    let door = ordo_host::IdentityAccess::sharing(
        Arc::new(directory.engine),
        TokenKey::from_secret(&SECRET),
        Box::new(ordo_host::runtime::now_secs),
        metrics.clone(),
    );
    let access = Arc::new(SwappableAccess::new(door));

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "door-behind-test",
        f.clone(),
        ordo_exec::Executor {
            pool: writer.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: space.clone(),
        },
        platform_ops::specs(),
    )
    .expect("the fixture compiles");
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(ordo_host::FixtureExecutor {
            spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
            pg,
            page_size: 50,
        }),
        access: access.clone(),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"door-behind-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        access,
        state,
        control,
        writer,
        reader,
        deployment,
        metrics,
        delegations,
        credentials,
        space: space.0,
    }
}

impl Harness {
    /// The rebuild a membership write runs before it answers.
    async fn rebuild(&self) -> Result<(), String> {
        ordo_host::runtime::refresh_access_for(
            &self.deployment,
            &self.control,
            &SECRET,
            &self.metrics,
            &self.access,
            self.delegations.clone(),
            &self.credentials,
            &self.space,
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// A read a viewer may make, as `who`: the status, `Retry-After`, body.
    async fn read_as(&self, token: &str) -> (StatusCode, Option<String>, Value) {
        let req = Request::builder()
            .method("GET")
            .uri("/op/activeProducts")
            .header("x-ordo-space", &self.space)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = router(self.state.clone()).oneshot(req).await.unwrap();
        let status = resp.status();
        let retry = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            retry,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

fn token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(&SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(9_000_000_000),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

fn assert_busy(what: &str, (status, retry, body): (StatusCode, Option<String>, Value)) {
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{what}: {body}");
    assert_eq!(body["error"]["code"], "server_busy", "{what}: {body}");
    assert!(retry.is_some(), "{what}: a 503 carries Retry-After");
}

/// A person admitted while the door's pool is held full is answered 503
/// with `Retry-After` on their first call, then 200 once a rebuild runs.
/// Before the fix the first call was 403 `access_denied`.
#[tokio::test(flavor = "multi_thread")]
async fn a_person_admitted_while_the_door_pool_is_full_is_told_to_retry_not_denied() {
    let h = harness("ordo_door_behind_held").await;
    let newcomer = "prn_door_behind_newcomer";

    let held = h.control.begin().await.expect("the one connection");
    MemberOps::admit_row(&h.writer, &h.space, newcomer, "viewer", "Newcomer")
        .await
        .expect("admitted")
        .expect("a new row");
    assert!(
        h.rebuild().await.is_err(),
        "the rebuild cannot run while the pool is held full"
    );
    assert_busy(
        "the newcomer's first call",
        h.read_as(&token(newcomer)).await,
    );
    // A token that proves nobody keeps the door's uniform denial.
    let (status, _, body) = h.read_as("not-a-token").await;
    assert_ne!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");

    drop(held);
    h.rebuild().await.expect("the rebuild runs");
    let (status, _, body) = h.read_as(&token(newcomer)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A database at its connection limit, read by a door with no good list
/// yet (just booted, or a tenant just made): 503, then 200 once it reads.
#[tokio::test(flavor = "multi_thread")]
async fn a_full_database_with_no_good_list_is_503_not_403() {
    let h = harness("ordo_door_behind_full").await;
    reader_limit(&h, 0).await;
    h.rebuild()
        .await
        .expect("the rebuild runs; the tenant is unread");
    assert_eq!(h.access.unread().len(), 1, "{:?}", h.access.unread());
    assert_busy("a member's call", h.read_as(&token(VIEWER)).await);

    reader_limit(&h, -1).await;
    h.rebuild().await.expect("the rebuild runs");
    let (status, _, body) = h.read_as(&token(VIEWER)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A full database is a passing failure (SQLSTATE 53300 is busy), so the
/// door keeps serving the last good list inside the grace. Before the fix
/// 53300 read as a lasting fault and the members were denied at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_full_database_keeps_the_last_good_members() {
    let h = harness("ordo_door_behind_keep").await;
    h.rebuild().await.expect("a good read");
    let (status, _, body) = h.read_as(&token(VIEWER)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    reader_limit(&h, 0).await;
    h.rebuild().await.expect("the rebuild runs");
    assert_eq!(
        h.access.kept_stale().len(),
        1,
        "served from the last good list"
    );
    let (status, _, body) = h.read_as(&token(VIEWER)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    reader_limit(&h, -1).await;
}
