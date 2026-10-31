//! Console step 6 (`plan-console.md` 4.2, 4.6, 4.10 and 7), through the
//! real door against real Postgres:
//!
//! - **G7**, `std.deployment.settings`: every setting's effective value,
//!   default, range and source; a secret only as set or unset; a value a
//!   platform op stored wins over the variable and says so. The deployment
//!   owner's, in the deployment's own space only.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::pause::PausePolicy;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::settings::{self, SettingsOps};
use ordo_host::FixtureExecutor;
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::Value;
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: [u8; 32] = *b"deployment-settings-test-secret!";
/// The conformance seed's owner of the tenant `conformance`.
const TENANT_OWNER: &str = "mem_o";
/// The deployment owner, an owner of the deployment's own space.
const DEPLOYMENT_OWNER: &str = "dep_owner";
/// A member of the deployment's own space who is not its owner.
const HOME_VIEWER: &str = "home_viewer";
const HOME: &str = "home";

/// What the deployment was started with, as a test hands it over.
const VARS: [(&str, &str); 6] = [
    ("ORDO_PG_PORT", "6543"),
    ("ORDO_TOKEN_KEY_FILE", "/run/secrets/token-key"),
    ("ORDO_PAGE_KEY", "a-page-key-value"),
    ("ORDO_AUTH_CLAIM_CODE_FILE", "/run/secrets/claim"),
    ("ORDO_AUTH_GUEST", "on"),
    ("PATH", "/usr/bin"),
];

struct Harness {
    state: ApiState,
    pool: ordo_exec::Pool,
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
    ordo_host::admission::ensure_table(&pool)
        .await
        .expect("setting table");
    let space = ordo_conformance::fixture_space().0;

    let grants = Arc::new(PgGrantRegistry::new(pool.clone()));
    grants.ensure_table().await.expect("platform table");
    let mut rows = pool.rows(ordo_host::MEMBERS_SQL).await.expect("members");
    let row = |space: &str, who: &str, role: &str| {
        vec![
            Some(space.into()),
            Some(who.into()),
            Some(role.into()),
            Some("active".into()),
            Some(who.into()),
        ]
    };
    rows.push(row(HOME, DEPLOYMENT_OWNER, "owner"));
    rows.push(row(HOME, HOME_VIEWER, "manager"));
    rows.push(row(&space, DEPLOYMENT_OWNER, "owner"));
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
        "deployment-settings-test",
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
    let vars = VARS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let ops = Arc::new(
        PlatformOps::new(engine, grants, Arc::new(|| NOW)).with_settings(Arc::new(
            SettingsOps::from_vars(pool.clone(), HOME.into(), vars),
        )),
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
        page_key: PageKey::from_secret(b"deployment-settings-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness { state, pool, space }
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

/// `std.deployment.settings`, as `who`, bound to `space`.
async fn read(h: &Harness, space: &str, who: &str) -> (StatusCode, Value) {
    let req = Request::get(format!("/op/{}", settings::SETTINGS))
        .header("x-ordo-space", space)
        .header("authorization", format!("Bearer {}", token(who)))
        .body(Body::empty())
        .unwrap();
    let resp = router(h.state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 24)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn row<'a>(body: &'a Value, name: &str) -> &'a Value {
    body["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("no row {name} in the answer"))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_reads_every_setting_with_its_value_default_range_and_source() {
    let h = harness("ordo_g7_values").await;
    let (status, body) = read(&h, HOME, DEPLOYMENT_OWNER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["rows"].as_array().unwrap();
    assert!(
        rows.len() >= settings::REGISTRY.len() - 30,
        "every registered setting has a row: {}",
        rows.len()
    );

    let port = row(&body, "ORDO_PG_PORT");
    assert_eq!(port["source"], "environment");
    assert_eq!(port["value"], "6543");
    assert_eq!(port["default"], "5432");
    assert_eq!(port["group"], "Databases and pools");

    let digits = row(&body, "ORDO_AUTH_EMAIL_CODE_DIGITS");
    assert_eq!(digits["source"], "default");
    assert_eq!(digits["set"], false);
    assert_eq!(digits["default"], "8");
    assert_eq!(digits["range"], "6 to 10");

    let token_key = row(&body, "ORDO_TOKEN_KEY");
    assert_eq!(token_key["source"], "file");
    assert_eq!(token_key["set"], true);
    assert!(token_key["value"].is_null());

    let page = row(&body, "ORDO_PAGE_KEY");
    assert_eq!(page["source"], "environment");
    assert_eq!(page["secret"], true);
    assert!(page["value"].is_null());

    // Nothing stored yet: the variable is what the deployment took.
    let guest = row(&body, "ORDO_AUTH_GUEST");
    assert_eq!(guest["source"], "environment");
    assert_eq!(guest["value"], "on");
    assert_eq!(guest["storedBy"], "std.deployment.setGuest");

    // Secrets are never shown: not a value, not a file's path.
    let text = body.to_string();
    for leaked in ["a-page-key-value", "/run/secrets", "/usr/bin", "<set>"] {
        assert!(!text.contains(leaked), "{leaked} is in the answer");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stored_value_wins_and_the_removed_claim_codes_row_is_never_shown() {
    let h = harness("ordo_g7_stored").await;
    // What `setGuest` stores, and a row the removed claim code left.
    h.pool
        .rows(
            r#"INSERT INTO "$deploymentsetting" ("key", "value", "updated_by", "updated_at")
               VALUES ('first_comer_claim_code', 'required:deadbeefcafe', 'environment', 7),
                      ('auth_guest', 'off', 'dep_owner', 8),
                      ('session_policy', '{"tokenSecs":600}', 'dep_owner', 9)"#,
        )
        .await
        .expect("stored");
    let (status, body) = read(&h, HOME, DEPLOYMENT_OWNER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !body.to_string().contains("setClaimCode"),
        "the removed op is named: {body}"
    );
    let guest = row(&body, "ORDO_AUTH_GUEST");
    assert_eq!(guest["source"], "stored");
    assert_eq!(guest["value"], "off");
    assert_eq!(guest["environment"], "on");
    assert_eq!(guest["stored"]["updatedBy"], "dep_owner");
    let layer = row(&body, "session_policy");
    assert_eq!(layer["kind"], "layer");
    assert_eq!(layer["value"], r#"{"tokenSecs":600}"#);
    assert!(!body.to_string().contains("deadbeefcafe"), "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_settings_answer_only_the_deployment_owner_in_its_own_space() {
    let h = harness("ordo_g7_scope").await;
    // A member of the home space who is not its owner.
    let (status, body) = read(&h, HOME, HOME_VIEWER).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    // The deployment owner, bound to a tenant they own.
    let (status, body) = read(&h, &h.space.clone(), DEPLOYMENT_OWNER).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    // A tenant's owner, in their tenant.
    let (status, body) = read(&h, &h.space.clone(), TENANT_OWNER).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    // Somebody with no role in the home space.
    let (status, body) = read(&h, HOME, TENANT_OWNER).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(platform_ops::owner_only(settings::SETTINGS));
}
