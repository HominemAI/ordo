//! K1b and K3 — the re-seal worker under the supervisor, and the two
//! gauges.
//!
//! `ordo-exec`'s suite proves the pass. This file proves the two things
//! the pass cannot know about: that the worker fills the `keys.reseal`
//! slot and drives a rotation to completion on an assembled deployment,
//! and that a pass which stops moving is visible as such — proved against
//! a deliberately wedged provider, not a mock.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::{Anchor, BlindValue, Epoch, KeyError, KeyLease, KeyProvider, NonceBase, Purpose};
use ordo_exec::reseal;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::metrics::{RESEAL_REMAINING, RESEAL_STUCK};
use ordo_host::runtime::{self, CustodyChoice, RuntimeConfig};
use ordo_host::supervisor::{Plan, Supervisor, Timing};
use ordo_host::workers::KeysReseal;

const SPACE: &str = "acme";
const PASSPHRASE: &str = "the reseal suite's passphrase";

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
        page_key: ordo_api::PageKey::from_secret(b"ordo-reseal-test"),
        checkpoint: None,
        security_checkpoint: false,
        apex_domain: None,
        cors: None,
        blob: None,
        exit_bundles: None,
        import_bundles: None,
        replica: "reseal-test".into(),
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
    let d = std::env::temp_dir().join(format!("ordo-reseal-{}", std::process::id()));
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
        ordo_crypto::master::seal_master(&path, "reseal-suite", PASSPHRASE.as_bytes())
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

/// The value of one gauge series in a Prometheus rendering.
fn gauge(render: &str, name: &str, labels: &str) -> Option<u64> {
    let key = format!("{name}{{{labels}}} ");
    render
        .lines()
        .find_map(|l| l.strip_prefix(&key))
        .and_then(|v| v.trim().parse().ok())
}

// ── K1b ──────────────────────────────────────────────────────────────────

/// **K1b's done-when.** A rotation enqueued on an assembled deployment is
/// driven to zero by the `keys.reseal` worker under the supervisor; the
/// memos written through the door read back unchanged through the door;
/// the census reports the worker enabled with units moved; the security
/// stream holds the completion; and the two gauges read zero.
#[tokio::test(flavor = "multi_thread")]
async fn a_rotation_reseals_under_the_supervisor() {
    const DB: &str = "ordo_rt_reseal_e2e";
    let pool = deployment_ready(DB).await;
    let rt = runtime::assemble(config(DB))
        .await
        .expect("the deployment assembles");
    let sup = rt.start_workers().await.expect("the supervisor starts");

    for (id, memo) in [("ord_1", "alpha"), ("ord_2", "beta"), ("ord_1", "gamma")] {
        call(
            &rt,
            "POST",
            "setMemo",
            serde_json::json!({ "order": id, "memo": memo }),
        )
        .await;
    }
    // Read through the deployment's own custody. The fixture's sealed
    // reads (`memoDigest`, `orderMemos`) plan as unbounded-host and the
    // door refuses them by design, so the ordinary read of a sealed column
    // is the provider opening the column's bytes, which is what a bounded
    // host-class read would do per row.
    let memos_before = vec![
        memo(&pool, rt.keys.as_ref(), "ord_1").await,
        memo(&pool, rt.keys.as_ref(), "ord_2").await,
    ];
    assert_eq!(
        memos_before,
        vec![Some("gamma".into()), Some("beta".into())]
    );

    let schema = rt.deployment.current().fixture.schema.clone();
    let old: Epoch = rt.keys.current_epoch(&space()).unwrap();
    let before = reseal::remaining(&pool, &schema, &space(), old)
        .await
        .unwrap();
    assert!(before.payloads >= 4 && before.columns == 2, "{before:?}");

    let new = reseal::enqueue_rotation(&pool, rt.keys.as_ref(), &space(), "suite", now())
        .await
        .expect("rotation");
    assert_eq!(new, old + 1);

    // Wait for the pass rows to close, which comes one re-count after the
    // last row moved: the worker's own notion of finished, not the test's.
    let moved = wait_for(Duration::from_secs(30), || {
        let pool = pool.clone();
        futures::executor::block_on(async move {
            reseal::passes(&pool, &space())
                .await
                .map(|rows| {
                    rows.len() == 4 && rows.iter().all(|r| r.state == reseal::PassState::Done)
                })
                .unwrap_or(false)
        })
    })
    .await;
    assert!(moved, "the pass did not finish; census: {}", sup.health());
    assert_eq!(
        reseal::remaining(&pool, &schema, &space(), old)
            .await
            .unwrap()
            .total(),
        0
    );

    // Through the deployment's custody, byte-identical.
    let memos_after = vec![
        memo(&pool, rt.keys.as_ref(), "ord_1").await,
        memo(&pool, rt.keys.as_ref(), "ord_2").await,
    ];
    assert_eq!(memos_after, memos_before);

    let health = sup.health();
    assert_eq!(health["keys.reseal"]["enabled"], serde_json::json!(true));
    assert!(
        health["keys.reseal"]["units"].as_u64().unwrap_or(0) >= before.total() as u64,
        "{health}"
    );
    assert_eq!(health["keys.reseal"]["failures"], serde_json::json!(0));

    // The pass rows are done and the gauges read zero.
    let rows = reseal::passes(&pool, &space()).await.unwrap();
    assert!(
        rows.iter().all(|r| r.state == reseal::PassState::Done),
        "{rows:?}"
    );
    let render = rt.metrics.render_prometheus();
    assert_eq!(
        gauge(
            &render,
            RESEAL_REMAINING,
            &format!("population=\"payloads\",space=\"{SPACE}\"")
        ),
        Some(0),
        "{render}"
    );
    assert_eq!(
        gauge(&render, RESEAL_STUCK, &format!("space=\"{SPACE}\"")),
        Some(0),
        "{render}"
    );

    // The completion was observed into the recorder the door drains, and
    // it lands in the space's own stream once the drain flushes. The
    // drain's tick is thirty seconds, so flush it the way `serve` does at
    // exit rather than wait: supervisor first, then the drain.
    assert!(
        rt.access.recorder.observed() >= 1,
        "the worker recorded nothing"
    );
    Arc::into_inner(sup)
        .expect("sole owner")
        .shutdown(Duration::from_secs(5))
        .await;
    let runtime::Runtime { secdrain, .. } = rt;
    let _ = secdrain.shutdown().await;
    let recorded = pool
        .rows(&format!(
            "SELECT count(*) FROM \"$secevents\" WHERE \"scope\" = '{SPACE}' \
             AND \"kind\" = 'custody-action' \
             AND position('reseal_completed' in encode(\"body\", 'escape')) > 0"
        ))
        .await
        .unwrap();
    assert_eq!(
        recorded[0][0].as_deref(),
        Some("1"),
        "no reseal_completed entry reached the space's stream"
    );
}

// ── K3 ───────────────────────────────────────────────────────────────────

/// A provider that refuses every `Reseal` lease and forwards everything
/// else: a pass over it reads its rows, cannot take the lease, and never
/// advances. That is the wedge, and it is what the stuck gauge is for.
struct RefusesReseal(Arc<dyn KeyProvider>);

impl KeyProvider for RefusesReseal {
    fn release(&self, s: &ordo_crypto::SpaceId, p: Purpose, c: &str) -> Result<KeyLease, KeyError> {
        if p == Purpose::Reseal {
            return Err(KeyError::NotLeased);
        }
        self.0.release(s, p, c)
    }
    fn seal_at(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.0.seal_at(l, e, f, a, v)
    }
    fn open_at(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.0.open_at(l, e, f, a, v)
    }
    #[allow(clippy::too_many_arguments)]
    fn seal_chunk(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        ep: Epoch,
        b: &NonceBase,
        c: u32,
        last: bool,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.0.seal_chunk(l, e, f, a, ep, b, c, last, v)
    }
    #[allow(clippy::too_many_arguments)]
    fn open_chunk(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        ep: Epoch,
        b: &NonceBase,
        c: u32,
        last: bool,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.0.open_chunk(l, e, f, a, ep, b, c, last, v)
    }
    fn blind_index(
        &self,
        s: &ordo_crypto::SpaceId,
        e: &str,
        f: &str,
        v: BlindValue<'_>,
    ) -> Result<[u8; 32], KeyError> {
        self.0.blind_index(s, e, f, v)
    }
    fn current_epoch(&self, s: &ordo_crypto::SpaceId) -> Result<Epoch, KeyError> {
        self.0.current_epoch(s)
    }
    fn rotate(&self, s: &ordo_crypto::SpaceId, r: &str) -> Result<Epoch, KeyError> {
        self.0.rotate(s, r)
    }
    fn create_space(&self, s: &ordo_crypto::SpaceId) -> Result<(), KeyError> {
        self.0.create_space(s)
    }
    fn destroy_epoch(&self, s: &ordo_crypto::SpaceId, e: Epoch) -> Result<(), KeyError> {
        self.0.destroy_epoch(s, e)
    }
    fn destroy_space(&self, s: &ordo_crypto::SpaceId) -> Result<(), KeyError> {
        self.0.destroy_space(s)
    }
}

/// **K3's done-when, the stuck half.** A pass whose cursor does not move
/// raises `ordo_reseal_stuck_seconds{space}` while
/// `ordo_reseal_rows_remaining{space,population="payloads"}` is non-zero,
/// against a worker that is genuinely wedged: its lease is refused, every
/// run fails, and the supervisor backs off between them.
#[tokio::test(flavor = "multi_thread")]
async fn a_wedged_pass_raises_the_stuck_gauge() {
    const DB: &str = "ordo_rt_reseal_stuck";
    let admin = Pool::connect(pg_cfg("postgres")).await.expect("pool");
    let _ = admin.rows(&format!("CREATE DATABASE {DB}")).await;
    let pool = Pool::connect(pg_cfg(DB)).await.expect("pool");
    pool.ensure_platform_tables().await.expect("tables");
    reseal::ensure_tables(&pool).await.expect("$reseal");
    let _ = pool.rows("DELETE FROM \"$reseal\"").await;
    let _ = pool.rows("DELETE FROM \"$sealedpayload\"").await;

    let honest: Arc<dyn KeyProvider> = Arc::new(ordo_crypto::TestProvider::with_space(SPACE));
    // One payload under epoch 1, written the way the write protocol writes
    // one: an envelope AAD-bound to its column, under a random handle.
    let lease = honest
        .release(&space(), Purpose::Write, "seed")
        .expect("lease");
    let old = honest.current_epoch(&space()).unwrap();
    let handle = honest.mint_sealed_ref(&lease, old).unwrap();
    let envelope = honest
        .seal_at(&lease, "orders", "memo", Anchor::Row("ord_1"), b"wedged")
        .unwrap();
    pool.rows(&ordo_crypto::sealed::insert_sql(
        &space(),
        &handle,
        old,
        "orders",
        "memo",
        "row:ord_1",
        "cg_seed",
        &envelope,
    ))
    .await
    .expect("seeded");

    let keys: Arc<dyn KeyProvider> = Arc::new(RefusesReseal(honest.clone()));
    let new = reseal::enqueue_rotation(&pool, keys.as_ref(), &space(), "suite", now())
        .await
        .unwrap();
    assert_eq!(new, old + 1);

    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let deployment =
        ordo_host::deploy::Deployment::empty(space(), pool.clone(), keys.clone()).expect("empty");
    let worker = Arc::new(KeysReseal {
        chunk: 8,
        pause: Duration::from_secs(1),
        stream: None,
        stores: None,
    });
    let sup = Supervisor::start(
        pool.clone(),
        keys,
        deployment,
        metrics.clone(),
        space(),
        pg_cfg(DB),
        Plan {
            workers: vec![worker],
            timing: Timing {
                backoff_unit: Duration::from_millis(100),
                leader_tick: Duration::from_millis(200),
                census_every: Duration::from_secs(60),
            },
            ..Plan::default()
        },
    )
    .await
    .expect("the supervisor starts");

    let stuck = wait_for(Duration::from_secs(20), || {
        let render = metrics.render_prometheus();
        gauge(&render, RESEAL_STUCK, &format!("space=\"{SPACE}\"")).unwrap_or(0) >= 2
    })
    .await;
    let render = metrics.render_prometheus();
    assert!(stuck, "the stuck gauge did not rise:\n{render}");
    assert_eq!(
        gauge(
            &render,
            RESEAL_REMAINING,
            &format!("population=\"payloads\",space=\"{SPACE}\"")
        ),
        Some(1),
        "{render}"
    );
    // The worker is failing, not idle: the census counts the failures and
    // the pass row counts the attempts.
    let health = sup.health();
    assert!(
        health["keys.reseal"]["failures"].as_u64().unwrap_or(0) >= 1,
        "{health}"
    );
    let rows = reseal::passes(&pool, &space()).await.unwrap();
    let payloads = rows
        .iter()
        .find(|r| r.population == reseal::Population::Payloads)
        .unwrap();
    assert!(payloads.attempts >= 1, "{payloads:?}");
    assert!(payloads.cursor.is_none(), "{payloads:?}");
    // And nothing moved: the honest provider still opens the row under
    // the old epoch.
    let row = pool
        .rows(&ordo_crypto::sealed::lookup_sql(&space(), &handle))
        .await
        .unwrap();
    let stored = ordo_crypto::sealed::payload_of(&row[0]).unwrap();
    assert_eq!(stored.epoch, old);
    assert_eq!(
        honest
            .open_at(
                &lease,
                "orders",
                "memo",
                Anchor::parse(&stored.anchor).unwrap(),
                &stored.bytes
            )
            .unwrap(),
        b"wedged"
    );

    sup.shutdown(Duration::from_secs(5)).await;
}
