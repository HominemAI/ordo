//! 25ab (`key-release-unrecorded`): the custody a deployed server serves
//! with is the metered one.
//!
//! The runtime used to hand every component the bare provider
//! `build_custody` returned. Only tests wrapped it in `MeteredKeys`, so a
//! deployed server counted no release on `ordo_key_release_total` and
//! recorded no `KeyReleased` event, and the release-breadth alert read
//! nothing. These tests assemble the runtime the binary assembles and
//! drive it through the door and the supervisor.
//!
//! A regression shows as a release delta of 0 (unmetered) or 2 (wrapped
//! twice) for one served op, or as no `key-released` row in the tenant's
//! own stream after the drain flushes.
//!
//! Needs the conformance cluster. Its own databases, prefixed
//! `ordo_rt_keyrel_`.

use std::sync::Arc;
use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::{Anchor, KeyProvider, Purpose};
use ordo_exec::reseal;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::metrics::KEY_RELEASES;
use ordo_host::runtime::{self, CustodyChoice, RuntimeConfig};

const SPACE: &str = "acme";
const PASSPHRASE: &str = "the key release suite's passphrase";

fn pg_cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 6,
        ..PoolConfig::from_env()
    }
}

fn config(db: &str) -> RuntimeConfig {
    RuntimeConfig {
        blob_routing: ordo_host::runtime::BlobRouting::Single,
        pg: pg_cfg(db),
        addr: "127.0.0.1:0".parse().expect("addr"),
        shutdown_grace: Duration::from_secs(5),
        space: ordo_crypto::SpaceId(SPACE.into()),
        custody: CustodyChoice::SealedMaster {
            path: key_file(),
            passphrase: pass_file(),
        },
        token_secret: [7u8; 32],
        page_key: ordo_api::PageKey::from_secret(b"ordo-keyrel-test"),
        checkpoint: None,
        security_checkpoint: false,
        apex_domain: None,
        cors: None,
        blob: None,
        exit_bundles: None,
        import_bundles: None,
        replica: "keyrel-test".into(),
        usage_flush: Duration::from_secs(60),
        usage_rollup: Duration::from_secs(3600),
        sync: Default::default(),
        auth: None,
        credentials: None,
        presence: Default::default(),
        pausing: Default::default(),
        delegable: Default::default(),
        access_template: Default::default(),
        deployment_owners: Vec::new(),
        provisioning: Default::default(),
        recovery: Default::default(),
        second_factor: Default::default(),
        mail: None,
    }
}

fn dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ordo-keyrel-{}", std::process::id()));
    std::fs::create_dir_all(&d).expect("tmp");
    d
}

fn key_file() -> std::path::PathBuf {
    dir().join("master.key")
}

/// Written once per test binary. Tests run in parallel, and a rewrite
/// truncates the file before it fills it, so a second test could read it
/// empty while the first was writing.
fn pass_file() -> std::path::PathBuf {
    static ONCE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let p = dir().join("pass");
        std::fs::write(&p, PASSPHRASE).expect("write");
        p
    })
    .clone()
}

fn master() -> [u8; 32] {
    use std::sync::OnceLock;
    static ONCE: OnceLock<[u8; 32]> = OnceLock::new();
    *ONCE.get_or_init(|| {
        let path = key_file();
        let _ = std::fs::remove_file(&path);
        ordo_crypto::master::seal_master(&path, "keyrel-suite", PASSPHRASE.as_bytes())
            .expect("sealed");
        ordo_crypto::master::open_sealed_master(&path, PASSPHRASE.as_bytes()).expect("opens")
    })
}

/// A deployment an operator has applied and `keys init`ed, under a master
/// key this test holds. The shape `tests/supervisor.rs` uses.
async fn deployment_ready(db: &str) -> Pool {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    let source = include_str!("../../ordo-conformance/fixtures/market.ordo");

    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("clean");
    let plain = Fixture::from_source(source, "").expect("compiles");
    pg.exec(&plain.ddl).expect("ddl");
    pg.exec(ordo_crypto::SEALED_PAYLOAD_DDL).expect("payloads");

    let pool = Pool::connect(pg_cfg(db)).await.expect("pool");
    ordo_exec::keystore::ensure_tables(&pool)
        .await
        .expect("custody table");
    let provider = ordo_crypto::LocalKeyProvider::from_master(
        master(),
        Box::new(ordo_exec::keystore::PgKeyStore::open(pg_cfg(db)).expect("store")),
    );
    provider
        .create_space(&ordo_crypto::SpaceId(SPACE.into()))
        .expect("custody");
    let keys: Arc<dyn ordo_crypto::KeyProvider> = Arc::new(provider);

    let mut f = Fixture::load().expect("fixture");
    f.keys = keys.clone();
    let f = f.in_space(SPACE);
    f.seed_into(&pg).expect("seed");

    ordo_exec::registry::ensure_tables(&pool)
        .await
        .expect("registry");
    let doc = ordo_plan::artifact::program_artifact(&f.schema, &f.registry).expect("artifact");
    let ddl = f.ddl.clone();
    let art = ordo_exec::registry::Artifact {
        version: ordo_plan::artifact::program_version(&doc, &ddl),
        artifact: ordo_plan::artifact::to_bytes(&doc),
        source: source.to_string(),
        ddl,
        applied_at: ordo_commandlog::Timestamp(1),
        actor: "operator:k1".into(),
    };
    ordo_exec::registry::store(&pool, &art)
        .await
        .expect("store");
    ordo_exec::registry::activate(
        &pool,
        &art.version,
        "operator:k1",
        ordo_commandlog::Timestamp(2),
    )
    .await
    .expect("activate");
    pool
}

fn token(principal: &str) -> String {
    ordo_identity::mint(
        &ordo_identity::TokenKey::from_secret(&[7u8; 32]),
        &ordo_identity::TokenSpec {
            principal: ordo_identity::principal::PrincipalId(principal.into()),
            expires: ordo_identity::principal::Timestamp(u64::MAX / 2),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

async fn call(
    rt: &runtime::Runtime,
    method: &str,
    op: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    let token = token("mem_1");
    let resp = ordo_api::router(rt.api.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!("/op/{op}"))
                .header("x-ordo-space", SPACE)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(st, 200, "{op}: {}", String::from_utf8_lossy(&bytes));
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

async fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    done()
}

fn space() -> ordo_crypto::SpaceId {
    ordo_crypto::SpaceId(SPACE.into())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The `orders.memo` column on a row, opened under a read release.
async fn memo(pool: &Pool, keys: &dyn KeyProvider, id: &str) -> Option<String> {
    let rows = pool
        .rows(&format!(
            "SELECT encode(\"memo\", 'hex') FROM \"orders\" WHERE \"$space\" = '{SPACE}' \
             AND \"id\" = '{id}'"
        ))
        .await
        .unwrap();
    let hex = rows.first()?.first()?.clone()?;
    let bytes = ordo_crypto::sealed::unhex(&format!("\\x{hex}"))?;
    if bytes.is_empty() {
        return None;
    }
    let lease = keys.release(&space(), Purpose::Read, "suite").ok()?;
    let plain = keys
        .open_at(&lease, "orders", "memo", Anchor::Row(id), &bytes)
        .ok()?;
    String::from_utf8(plain).ok()
}

/// Releases counted for `space` under `purpose`, read from the series the
/// `/metrics` route renders.
fn releases(rt: &runtime::Runtime, space: &str, purpose: &str) -> u64 {
    rt.metrics
        .snapshot()
        .into_iter()
        .find(|(name, labels, _)| {
            name == KEY_RELEASES
                && labels.iter().any(|(k, v)| k == "space" && v == space)
                && labels.iter().any(|(k, v)| k == "purpose" && v == purpose)
        })
        .map(|(_, _, n)| n)
        .unwrap_or(0)
}

/// `key-released` rows in one scope's stream whose body names `purpose`.
async fn recorded(pool: &Pool, scope: &str, purpose: &str) -> u64 {
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM \"$secevents\" WHERE \"scope\" = '{scope}' \
             AND \"kind\" = 'key-released' \
             AND position('{purpose}' in encode(\"body\", 'escape')) > 0"
        ))
        .await
        .unwrap();
    rows[0][0].as_deref().unwrap_or("0").parse().unwrap()
}

/// A sealed write and a sealed read served through the assembled door are
/// one release each, counted on the metric the `/metrics` route renders
/// and recorded as `KeyReleased` in the tenant's own stream.
#[tokio::test(flavor = "multi_thread")]
async fn a_served_sealed_write_and_read_are_counted_and_recorded() {
    const DB: &str = "ordo_rt_keyrel_door";
    let pool = deployment_ready(DB).await;
    let rt = runtime::assemble(config(DB))
        .await
        .expect("the deployment assembles");

    let writes = releases(&rt, SPACE, "write");
    call(
        &rt,
        "POST",
        "setMemo",
        serde_json::json!({ "order": "ord_1", "memo": "sealed-on-the-door" }),
    )
    .await;
    // A sealed field set is two write releases by design: one seals the
    // value, one mints the handle the command image names it by
    // (`seal_image_ref`). 0 is unmetered custody; 4 is custody wrapped
    // twice.
    assert_eq!(
        releases(&rt, SPACE, "write") - writes,
        2,
        "one sealed write: a seal release and an image release"
    );

    let reads = releases(&rt, SPACE, "read");
    let body = call(&rt, "GET", "memoDigest", serde_json::Value::Null).await;
    assert!(
        body.to_string().contains("sealed-on-the-door"),
        "the sealed read is served in plaintext: {body}"
    );
    assert_eq!(
        releases(&rt, SPACE, "read") - reads,
        1,
        "one sealed page is one read release"
    );

    // The series is the one the route renders.
    let render = rt.metrics.render_prometheus();
    assert!(
        render.contains(&format!(
            "{KEY_RELEASES}{{purpose=\"read\",space=\"{SPACE}\"}}"
        )),
        "{render}"
    );

    // Flushed the way `serve` does at exit, then read from the database:
    // each release lands in the space's own stream.
    let runtime::Runtime { secdrain, .. } = rt;
    let _ = secdrain.shutdown().await;
    assert!(
        recorded(&pool, SPACE, "write").await >= 1,
        "no write release reached the tenant's stream"
    );
    assert!(
        recorded(&pool, SPACE, "read").await >= 1,
        "no read release reached the tenant's stream"
    );
}

/// The supervisor's workers hold the same metered custody as the door: a
/// rotation driven to completion by `keys.reseal` counts `reseal`
/// releases for the space and records them in its stream.
#[tokio::test(flavor = "multi_thread")]
async fn the_reseal_worker_releases_through_the_metered_custody() {
    const DB: &str = "ordo_rt_keyrel_reseal";
    let pool = deployment_ready(DB).await;
    let rt = runtime::assemble(config(DB))
        .await
        .expect("the deployment assembles");
    let sup = rt.start_workers().await.expect("the supervisor starts");

    call(
        &rt,
        "POST",
        "setMemo",
        serde_json::json!({ "order": "ord_1", "memo": "to-be-resealed" }),
    )
    .await;
    assert_eq!(releases(&rt, SPACE, "reseal"), 0);

    let schema = rt.deployment.current().fixture.schema.clone();
    let old = rt.keys.current_epoch(&space()).unwrap();
    reseal::enqueue_rotation(&pool, rt.keys.as_ref(), &space(), "suite", now())
        .await
        .expect("rotation");
    let moved = wait_for(Duration::from_secs(30), || {
        let pool = pool.clone();
        let schema = schema.clone();
        futures::executor::block_on(async move {
            reseal::remaining(&pool, &schema, &space(), old)
                .await
                .map(|r| r.total() == 0)
                .unwrap_or(false)
        })
    })
    .await;
    assert!(moved, "the pass did not finish; census: {}", sup.health());
    assert!(
        releases(&rt, SPACE, "reseal") >= 1,
        "the worker resealed without a counted release: it holds bare custody"
    );

    // `rt.keys`, the handle a later deploy and `ordo dev` take, is metered
    // too: a release through it is counted.
    let reads = releases(&rt, SPACE, "read");
    assert_eq!(
        memo(&pool, rt.keys.as_ref(), "ord_1").await.as_deref(),
        Some("to-be-resealed"),
        "the resealed memo opens under the new epoch"
    );
    assert_eq!(releases(&rt, SPACE, "read") - reads, 1);

    Arc::into_inner(sup)
        .expect("sole owner")
        .shutdown(Duration::from_secs(5))
        .await;
    let runtime::Runtime { secdrain, .. } = rt;
    let _ = secdrain.shutdown().await;
    assert!(
        recorded(&pool, SPACE, "reseal").await >= 1,
        "no reseal release reached the tenant's stream"
    );
}

/// Every holder the runtime exposes shares the one metered handle, and it
/// is wrapped once: a single release through it counts 1, not 0 or 2, and
/// records one `KeyReleased` in the tenant's stream.
///
/// The executor's copy is the deployment fixture's; the workers, a later
/// deploy and `ordo dev` take `rt.keys`. The door and reseal tests above
/// cover the holders this one cannot name (the replay cache, the blob
/// pipeline, the platform ops), by what they release.
#[tokio::test(flavor = "multi_thread")]
async fn every_holder_shares_one_custody_wrapped_once() {
    const DB: &str = "ordo_rt_keyrel_holders";
    let pool = deployment_ready(DB).await;
    let rt = runtime::assemble(config(DB))
        .await
        .expect("the deployment assembles");

    assert!(
        Arc::ptr_eq(&rt.deployment.current().fixture.keys, &rt.keys),
        "the executor serves with custody other than the runtime's metered handle"
    );

    let before = releases(&rt, SPACE, "read");
    let lease = rt
        .keys
        .release(&space(), Purpose::Read, "holders-suite")
        .expect("release");
    drop(lease);
    assert_eq!(
        releases(&rt, SPACE, "read") - before,
        1,
        "0 is bare custody, 2 is custody wrapped twice"
    );

    let runtime::Runtime { secdrain, .. } = rt;
    let _ = secdrain.shutdown().await;
    assert!(
        recorded(&pool, SPACE, "read").await >= 1,
        "the release did not reach the tenant's stream"
    );
}
