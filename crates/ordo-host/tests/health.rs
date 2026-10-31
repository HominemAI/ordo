//! 17t — `/health/ready` answers "can this platform accept and run a
//! deployment", never "is a program running".
//!
//! Before this change, `health_ready` gated on the live registry's canary
//! op and its shape report, so a [`Deployment::empty`] deployment — a
//! database nobody has run `ordo apply` against yet, the ordinary first
//! minute of a fresh one — answered 503 forever. A Kubernetes readiness
//! probe pointed at it held the pod out of rotation permanently, which is
//! why the cluster manifest probed `/health/live` instead, with a comment
//! saying so.
//!
//! `a_deployment_with_no_program_applied_is_ready` is that regression,
//! pinned: run against the pre-17t handler it fails (`not_ready`, 503,
//! naming `registry` and `shape`), which is what "observe the first one
//! failing" asks for here.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use ordo_api::{AccessChecker, AccessDecision};
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::deploy::Deployment;
use ordo_host::metrics::Metrics;
use ordo_host::serve::{platform_router, PlatformState};
use serde_json::Value;
use tower::util::ServiceExt;

/// One blob field, not inline, so it needs a store — `blob_door.rs`'s and
/// `spaces.rs`'s entity, unchanged.
const DOCS_SCHEMA: &str = "
entity docs (key: doc) {
  owner -> members
  title:  text
  scan:   blob?
  access read   owner == $caller
  access write  owner == $caller
}
";

/// The deployment owner is the one token `owner`; everyone else is denied.
/// Anonymous probes get the status alone (audit F12), so most tests below
/// ask as the owner to read the detail, and one asks anonymously.
struct OwnerOnly;

#[async_trait::async_trait]
impl AccessChecker for OwnerOnly {
    fn check(&self, token: &str, _space: &str, op: &str) -> AccessDecision {
        if token == "owner" && op == ordo_host::serve::HEALTH_DEEP_OP {
            AccessDecision::allow("owner")
        } else {
            AccessDecision::Deny
        }
    }
}

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    }
}

/// A [`PlatformState`] over a fixture compiled from `source`, on its own
/// database, with no supervisor and no blob store attached. Callers that
/// need one call [`PlatformState::with_blob_label`] on the result.
///
/// Returns the ACTUAL database name (`Pg::named` namespaces `db` per run),
/// for the one caller that needs to act on that same database again —
/// dropping it out from under the pool, say — without re-deriving a name
/// that would no longer match.
async fn platform_over(db: &str, source: &str) -> (PlatformState, String) {
    let pg = Pg::scoped(db);
    let f = Arc::new(Fixture::from_source(source, SEED).expect("the fixture compiles"));
    f.reset(&pg).expect("schema applied");
    let pool = Pool::connect(cfg(&pg.db)).await.expect("pool boots");
    let keys = f.keys.clone();
    let deployment = Deployment::pinned(
        "health-test",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    (
        PlatformState::new(
            Arc::new(OwnerOnly),
            deployment,
            pool,
            keys,
            ordo_host::DEFAULT_SPACE,
            Arc::new(Metrics::new()),
        ),
        pg.db,
    )
}

async fn get_path(app: axum::Router, path: &str, token: Option<&str>) -> (u16, Value) {
    let mut req = Request::builder().method("GET").uri(path);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

/// Readiness as the deployment owner, who reads the detail.
async fn get_ready(app: axum::Router) -> (u16, Value) {
    get_path(app, "/health/ready", Some("owner")).await
}

/// **17t's done-when, half one.** A deployment with no program applied
/// answers ready, and says so honestly about the program in the same
/// breath.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_with_no_program_applied_is_ready() {
    let db = "ordo_health_empty";
    let space = ordo_crypto::SpaceId(ordo_host::DEFAULT_SPACE.to_string());
    // `Deployment::empty`'s own doc names this exact case: a database
    // nobody has run `ordo apply` against yet. Its pool still needs to be a
    // real, reachable one — readiness is not exempted from the other
    // checks by having no program, it is exempted from needing one.
    let pg = Pg::scoped(db);
    pg.exec("select 1").expect("database reachable");
    let pool = Pool::connect(cfg(&pg.db)).await.expect("pool boots");
    let keys: Arc<dyn ordo_crypto::KeyProvider> = Arc::new(ordo_crypto::TestProvider::with_space(
        ordo_host::DEFAULT_SPACE,
    ));
    let deployment =
        Deployment::empty(space, pool.clone(), keys.clone()).expect("an empty deployment boots");

    // The regression, pinned directly against the fields the pre-17t
    // `health_ready` gated on (`crates/ordo-host/src/deploy.rs`'s
    // `empty_with_platform_ops` sets both by construction): a fresh
    // deployment's canary op is empty and it carries no shape report, so
    // the old `if canary_op.is_empty() || specs.get(&canary_op).is_none()
    // { failed.push("registry") }` and
    // `match report { Some(r) if ... => {}, _ => failed.push("shape") }`
    // both failed unconditionally, and `/health/ready` answered 503 for
    // every deployment with nothing applied yet — which is what this test
    // now proves is fixed.
    let live = deployment.current();
    assert!(
        live.canary_op.is_empty() && live.specs.get(&live.canary_op).is_none(),
        "sanity: an empty deployment has no canary op — the pre-17t \"registry\" check"
    );
    assert!(
        live.report.is_none(),
        "sanity: an empty deployment carries no shape report — the pre-17t \"shape\" check"
    );

    let platform = PlatformState::new(
        Arc::new(OwnerOnly),
        deployment,
        pool,
        keys,
        ordo_host::DEFAULT_SPACE,
        Arc::new(Metrics::new()),
    );
    let app = platform_router(platform);

    let (status, body) = get_ready(app).await;
    assert_eq!(
        status, 200,
        "a correctly configured empty deployment must answer ready: {body}"
    );
    assert_eq!(body["status"], "ready", "{body}");
    assert_eq!(
        body["program_active"], false,
        "no program has been applied, and the body must say so: {body}"
    );
}

/// **17t's done-when, half two.** A deployment whose database cannot be
/// reached is not ready, and a program being active does not change that —
/// the two facts are independent, which is the whole point of moving
/// "active" into the body.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_whose_database_is_unreachable_is_not_ready() {
    let db = "ordo_health_unreachable";
    let (platform, actual_db) = platform_over(db, FIXTURE).await;
    let app = platform_router(platform);

    // Drop the database out from under the pool's own open connection.
    // `WITH (FORCE)` (PG 13+) terminates any remaining backend itself, so
    // by the time this returns the pool's connection is dead and the
    // target it would redial no longer exists either — the next query
    // fails one way or the other, which is what "unreachable" needs to
    // mean here rather than "this one socket happened to be reused".
    // `actual_db`, not `db`: `platform_over` namespaced it per run.
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    admin
        .exec(&format!("DROP DATABASE {actual_db} WITH (FORCE)"))
        .expect("the database drops out from under the pool");

    let (status, body) = get_ready(app).await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["status"], "not_ready", "{body}");
    assert_eq!(
        body["checks"]["database"]["ok"], false,
        "the database check must name the failure: {body}"
    );
    assert_eq!(
        body["program_active"], true,
        "the market fixture is a live program, unaffected by the database being down: {body}"
    );
}

/// **17t's done-when, half three.** A deployment missing a required
/// credential — here, a blob store the live program's schema actually
/// needs — is not ready, and the body names which.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_missing_a_required_credential_is_not_ready_and_names_it() {
    let db = "ordo_health_blob_missing";
    let source = format!("{FIXTURE}\n{DOCS_SCHEMA}");
    // No `.with_blob_label(...)`: this deployment attached no blob store,
    // and its live program declares `docs.scan` as a non-inline `blob`.
    let (platform, _db) = platform_over(db, &source).await;
    let app = platform_router(platform);

    let (status, body) = get_ready(app).await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["status"], "not_ready", "{body}");
    assert_eq!(body["checks"]["config"]["ok"], false, "{body}");
    let detail = body["checks"]["config"]["detail"]
        .as_str()
        .unwrap_or_default();
    assert!(
        detail.contains("docs.scan"),
        "the body must name the field that has nowhere to put its bytes: {detail}"
    );
    assert_eq!(
        body["program_active"], true,
        "a program is loaded and serving even though it cannot accept every write: {body}"
    );
}

/// The mirror of the credential test: the same program, with a blob store
/// attached, is ready. Without this, a bug that always failed `config`
/// would pass the test above for the wrong reason.
#[tokio::test(flavor = "multi_thread")]
async fn the_same_program_is_ready_once_a_blob_store_is_attached() {
    let db = "ordo_health_blob_present";
    let source = format!("{FIXTURE}\n{DOCS_SCHEMA}");
    let (platform, _db) = platform_over(db, &source).await;
    let platform = platform.with_blob_label(Some("fs:/tmp/ordo-health-test".to_string()));
    let app = platform_router(platform);

    let (status, body) = get_ready(app).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "ready", "{body}");
    assert_eq!(body["checks"]["config"]["ok"], true, "{body}");
    assert_eq!(body["program_active"], true, "{body}");
}

/// **Audit F12.** An anonymous asker gets the status and nothing else from
/// `/health/ready` and `/health/deploy`; the deployment owner's token gets
/// the detail.
#[tokio::test(flavor = "multi_thread")]
async fn anonymous_health_answers_status_only() {
    let (platform, _db) = platform_over("ordo_health_anonymous", FIXTURE).await;
    let app = platform_router(platform);

    let (status, body) = get_path(app.clone(), "/health/ready", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, serde_json::json!({ "status": "ready" }), "{body}");

    let (_, owner) = get_path(app.clone(), "/health/ready", Some("owner")).await;
    for key in ["checks", "failed", "quarantined", "credential_labels"] {
        assert!(owner.get(key).is_some(), "the owner reads `{key}`: {owner}");
    }

    let (status, body) = get_path(app.clone(), "/health/deploy", None).await;
    assert_eq!(status, 200, "{body}");
    let keys: Vec<&String> = body.as_object().expect("an object").keys().collect();
    assert_eq!(keys, ["settled", "status"], "{body}");

    let (_, owner) = get_path(app, "/health/deploy", Some("owner")).await;
    for key in ["version", "nodes", "pending"] {
        assert!(owner.get(key).is_some(), "the owner reads `{key}`: {owner}");
    }
}
