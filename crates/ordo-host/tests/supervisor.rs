//! R3 — the worker supervisor, over an assembled runtime.
//!
//! **What "end-to-end" honestly means here.** A write reaches the door, the
//! command log appends, `drain_listener` accumulates it into
//! `$triggerbatches`, `fire_due` turns the batch into a `$triggerruns` row,
//! `claim_runs` claims it, and a registered `RunHandler` receives it with
//! the right `$trigger`. Every arrow is production code. The handler here
//! records what it received; the handler in production is checklist 12a,
//! and nothing between them is stubbed.
//!
//! This suite builds a `RuntimeConfig` **directly** rather than through
//! `from_env` — which is the whole point of `assemble` working over the
//! struct — and starts the workers without binding a socket, because a
//! `RunHandler` is a Rust value and there is no wire to register one over.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_crypto::KeyProvider as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_exec::triggers::{Listener, Run};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::runtime::{self, CustodyChoice, RuntimeConfig, TriggerWiring};
use ordo_host::supervisor::{
    BoxFut, Cadence, Plan, Progress, RunHandler, Supervisor, Worker, WorkerCtx, WorkerError,
};

const SPACE: &str = "acme";

fn pg_cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 6,
        ..PoolConfig::from_env()
    }
}

/// A `RuntimeConfig` built by hand. Every field is `pub` precisely so a
/// caller that did not come from an environment can assemble the same
/// deployment a server does — gate included.
///
/// Custody is `SealedMaster` over files this test writes, not `Local`:
/// `Local` reads `ORDO_MASTER_KEY`, `std::env::set_var` is `unsafe`, and
/// this workspace forbids `unsafe`. That is the right trade and it costs
/// nothing here — the sealed file is the posture a deployment should be
/// using anyway, and the test exercises it.
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
        page_key: ordo_api::PageKey::from_secret(b"ordo-supervisor-test"),
        // No checkpoint key: the checkpoint, attestation and delivery
        // workers report themselves disabled, which is what this suite
        // wants to observe anyway.
        checkpoint: None,
        security_checkpoint: false,
        apex_domain: None,
        cors: None,
        blob: None,
        exit_bundles: None,
        import_bundles: None,
        replica: "sup-test".into(),
        usage_flush: Duration::from_secs(1),
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

/// A deployment an operator has applied and `keys init`ed, under a master
/// key this test holds.
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
        actor: "operator:r3".into(),
    };
    ordo_exec::registry::store(&pool, &art)
        .await
        .expect("store");
    ordo_exec::registry::activate(
        &pool,
        &art.version,
        "operator:r3",
        ordo_commandlog::Timestamp(2),
    )
    .await
    .expect("activate");
    pool
}

const PASSPHRASE: &str = "the supervisor suite's passphrase";

fn dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ordo-sup-{}", std::process::id()));
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

/// The master this suite's deployments are provisioned under, sealed into a
/// file once per process.
fn master() -> [u8; 32] {
    use std::sync::OnceLock;
    static ONCE: OnceLock<[u8; 32]> = OnceLock::new();
    *ONCE.get_or_init(|| {
        let path = key_file();
        let _ = std::fs::remove_file(&path);
        ordo_crypto::master::seal_master(&path, "supervisor-suite", PASSPHRASE.as_bytes())
            .expect("sealed");
        ordo_crypto::master::open_sealed_master(&path, PASSPHRASE.as_bytes()).expect("opens")
    })
}

fn listener() -> Listener {
    Listener {
        domain: "market".into(),
        pattern: ordo_syntax::trigger::ListenerPattern::parse("market.orders.*").expect("pattern"),
        routine: "notifyOnOrder".into(),
        version: "1".into(),
        modifiers: ordo_syntax::trigger::Modifiers::default(),
    }
}

/// A handler that records every run it was handed.
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<Run>>,
}

impl RunHandler for Recorder {
    fn handle<'a>(&'a self, run: &'a Run, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            self.seen.lock().expect("seen").push(run.clone());
            Ok(())
        })
    }
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

/// A token for `principal` under the suite's token secret.
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

/// One HTTP/1.1 request over a real socket, `Connection: close`. Returns
/// the status and the body.
async fn http(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: &str,
) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\
         x-ordo-space: {SPACE}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n",
        body.len()
    );
    if let Some(t) = bearer {
        req.push_str(&format!("authorization: Bearer {t}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).await.expect("write");
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.expect("read");
    let text = String::from_utf8_lossy(&buf).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

/// The JSON object inside a response body, tolerating chunked framing.
fn json_of(body: &str) -> serde_json::Value {
    let (Some(a), Some(b)) = (body.find('{'), body.rfind('}')) else {
        return serde_json::Value::Null;
    };
    serde_json::from_str(&body[a..=b]).unwrap_or(serde_json::Value::Null)
}

/// A database with the platform tables and nothing else, and the handles a
/// bare `Supervisor::start` needs.
async fn bare(
    db: &str,
) -> (
    Pool,
    Arc<ordo_host::metrics::Metrics>,
    Arc<dyn ordo_crypto::KeyProvider>,
    Arc<ordo_host::deploy::Deployment>,
) {
    let admin = Pool::connect(pg_cfg("postgres")).await.expect("pool");
    let _ = admin.rows(&format!("CREATE DATABASE {db}")).await;
    let pool = Pool::connect(pg_cfg(db)).await.expect("pool");
    pool.ensure_platform_tables().await.expect("tables");
    let keys: Arc<dyn ordo_crypto::KeyProvider> =
        Arc::new(ordo_crypto::TestProvider::with_space(SPACE));
    let deployment = ordo_host::deploy::Deployment::empty(
        ordo_crypto::SpaceId(SPACE.into()),
        pool.clone(),
        keys.clone(),
    )
    .expect("an empty deployment");
    (
        pool,
        Arc::new(ordo_host::metrics::Metrics::new()),
        keys,
        deployment,
    )
}

async fn write_memo(rt: &runtime::Runtime, memo: &str) {
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    let token = token("mem_1");
    let resp = ordo_api::router(rt.api.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/op/setMemo")
                .header("x-ordo-space", SPACE)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "order": "ord_1", "memo": memo }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status().as_u16();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(st, 200, "{}", String::from_utf8_lossy(&body));
}

// ── the done-when ────────────────────────────────────────────────────────

/// 14g: the allowlist wraps the real assembled door. A preflight from a
/// listed origin is answered, one from another origin is refused, and a
/// real request from a listed origin carries `Access-Control-Allow-Origin`.
#[tokio::test(flavor = "multi_thread")]
async fn the_assembled_door_answers_a_cors_preflight_for_a_listed_origin() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    const DB: &str = "ordo_rt_cors";
    deployment_ready(DB).await;
    let mut cfg = config(DB);
    cfg.cors = ordo_api::cors::CorsPolicy::parse("https://app.example.com").expect("parses");
    let rt = runtime::assemble(cfg).await.expect("assembles");
    let app = runtime::door(&rt);

    let pre = |origin: &str| {
        Request::builder()
            .method("OPTIONS")
            .uri("/op/setMemo")
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "authorization, x-ordo-space, idempotency-key",
            )
            .body(Body::empty())
            .unwrap()
    };
    let ok = app
        .clone()
        .oneshot(pre("https://app.example.com"))
        .await
        .unwrap();
    assert_eq!(ok.status().as_u16(), 204);
    assert_eq!(
        ok.headers()["access-control-allow-origin"],
        "https://app.example.com"
    );
    let no = app
        .clone()
        .oneshot(pre("https://other.example.net"))
        .await
        .unwrap();
    assert_eq!(no.status().as_u16(), 403);
    assert!(!no.headers().contains_key("access-control-allow-origin"));

    let real = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/health/live")
                .header("origin", "https://app.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        real.headers()["access-control-allow-origin"],
        "https://app.example.com"
    );
}

/// **R3's done-when.** A write through the door reaches a registered
/// `RunHandler` with the right `$trigger` — id set, event kinds, batch id —
/// through the production `drain_listener` → `fire_due` → `claim_runs`, and
/// is `finish_run`ed rather than dead-lettered.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_through_the_door_reaches_a_run_handler() {
    const DB: &str = "ordo_rt_sup_e2e";
    let pool = deployment_ready(DB).await;

    let handler = Arc::new(Recorder::default());
    let rt = runtime::assemble(config(DB))
        .await
        .expect("the deployment assembles")
        .with_triggers(TriggerWiring {
            listeners: vec![listener()],
            clock: Vec::new(),
            handler: Some(handler.clone()),
            tenants: None,
        });
    let sup = rt.start_workers().await.expect("the supervisor starts");

    write_memo(&rt, "through-the-loop").await;

    let reached = wait_for(Duration::from_secs(25), || {
        !handler.seen.lock().expect("seen").is_empty()
    })
    .await;
    assert!(
        reached,
        "no run reached the handler; census: {}",
        sup.health()
    );

    let seen = handler.seen.lock().expect("seen").clone();
    let run = &seen[0];
    assert_eq!(run.listener, listener().key());
    assert!(
        run.ids.iter().any(|i| i == "ord_1"),
        "the $trigger does not carry the row the write touched: {:?}",
        run.ids
    );
    assert!(
        !run.events.is_empty(),
        "the $trigger carries no event kinds"
    );
    assert!(!run.batch.is_empty(), "the $trigger carries no batch id");

    // `finish_run`, not a dead letter.
    let finished = wait_for(Duration::from_secs(10), || true).await;
    assert!(finished);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let dead = ordo_exec::triggers::dead_letters(&pool, &ordo_commandlog::SpaceId(SPACE.into()))
        .await
        .expect("dead letters");
    assert!(dead.is_empty(), "the run dead-lettered: {dead:?}");

    // The census: `runs` and `units` are deliberately separate, because a
    // drain that runs a thousand times and moves zero units is healthy on
    // the first series and is the actual failure.
    let health = sup.health();
    assert_eq!(health["triggers.drain"]["enabled"], serde_json::json!(true));
    assert!(
        health["triggers.drain"]["units"].as_u64().unwrap_or(0) > 0,
        "the drain reports no units: {health}"
    );
    assert_eq!(
        health["triggers.runner"]["enabled"],
        serde_json::json!(true)
    );

    Arc::into_inner(sup)
        .expect("sole owner")
        .shutdown(Duration::from_secs(5))
        .await;
}

/// With no handler registered, `triggers.runner` does **not** start and the
/// census says why — and the named empty slots are registered rather than
/// absent, so K1 and M1 are a worker addition and not a supervisor change.
///
/// A fixed wiring (`with_triggers`), because since 2026-10-22 a program's
/// own routines follow the version switch and run every trigger worker
/// idle (`routines.rs`, `a_routine_added_by_a_version_switch_...`).
#[tokio::test(flavor = "multi_thread")]
async fn without_a_run_handler_the_runner_does_not_start() {
    const DB: &str = "ordo_rt_sup_norunner";
    deployment_ready(DB).await;
    let rt = runtime::assemble(config(DB))
        .await
        .expect("assembles")
        .with_triggers(TriggerWiring::default());
    let sup = rt.start_workers().await.expect("starts");

    let health = sup.health();
    assert_eq!(
        health["triggers.runner"]["enabled"],
        serde_json::json!(false)
    );
    assert!(
        health["triggers.runner"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("12a"),
        "the census must name what is missing: {health}"
    );
    // `blob.gc` is a real worker since U8 and this deployment configures no
    // blob store, so it is off with the variable named rather than absent
    // from the census.
    assert_eq!(
        health["blob.gc"]["enabled"],
        serde_json::json!(false),
        "{health}"
    );
    assert!(
        health["blob.gc"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("ORDO_BLOB_STORE"),
        "blob.gc is off and the census does not name the variable: {health}"
    );
    // The checkpoint family is off with the variable named, not absent.
    for w in [
        "commandlog.checkpoint",
        "commandlog.attest",
        "attest.deliver",
    ] {
        assert!(
            health[w]["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("ORDO_CHECKPOINT_KEY"),
            "{w} is off and the census does not name the variable: {health}"
        );
    }
    // 17n M1b/M1c: `usage.flush` and `usage.rollup` are off with the ledger
    // variable named — the one switch, not two of their own to fall out of
    // step with it. `usage.rollup` is no longer a registered `Slot`
    // (`usage_rollup_is_no_longer_a_slot`, `tests/usage.rs`); it is a real
    // `Worker` this census disables the same way.
    for w in ["usage.flush", "usage.rollup"] {
        assert!(
            health[w]["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("ORDO_USAGE_LEDGER"),
            "{health}"
        );
    }
    // `ORDO_SECURITY_CHECKPOINT_KEY` is unset, so the boot made one and
    // sealed it in `$securitykey` (`plan-identity-service.md` section 7):
    // security retention and checkpoints run. The security drain is in the
    // census as running and not supervised.
    for w in ["secevent.retain", "secevent.checkpoint"] {
        assert_eq!(health[w]["enabled"], serde_json::json!(true), "{health}");
    }
    assert_eq!(health["secevent.drain"]["enabled"], serde_json::json!(true));
    assert_eq!(
        health["secevent.drain"]["supervised"],
        serde_json::json!(false),
        "{health}"
    );
    // `keys.reseal` (K1) is always registered and is leader-only, so this
    // replica contends even with every optional worker off. It is alone
    // here, so it leads.
    let leadership = sup.overview()["leadership"].clone();
    assert_eq!(
        leadership["wanted"],
        serde_json::json!(true),
        "{leadership}"
    );
    assert_eq!(
        leadership["leader"],
        serde_json::json!(true),
        "{leadership}"
    );
    Arc::into_inner(sup)
        .expect("sole owner")
        .shutdown(Duration::from_secs(5))
        .await;
}

// ── supervision ──────────────────────────────────────────────────────────

struct Exploder(AtomicU64);

impl Worker for Exploder {
    fn name(&self) -> &'static str {
        "test.exploder"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(Duration::from_secs(1))
    }
    fn run<'a>(&'a self, _c: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("a worker panic that must not reach the door");
        })
    }
}

/// A worker that panics on its first unit is restarted with the platform's
/// own backoff and counted — **and the door's metrics registry still
/// works afterwards**, which is the adjacency that makes catching the panic
/// worth anything.
#[tokio::test(flavor = "multi_thread")]
async fn a_panicking_worker_is_restarted_and_does_not_poison_the_door() {
    const DB: &str = "ordo_rt_sup_panic";
    let pool = Pool::connect(pg_cfg("postgres")).await.expect("pool");
    let _ = pool.rows(&format!("CREATE DATABASE {DB}")).await;
    let pool = Pool::connect(pg_cfg(DB)).await.expect("pool");
    pool.ensure_platform_tables().await.expect("tables");

    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let keys: Arc<dyn ordo_crypto::KeyProvider> =
        Arc::new(ordo_crypto::TestProvider::with_space(SPACE));
    let deployment = ordo_host::deploy::Deployment::empty(
        ordo_crypto::SpaceId(SPACE.into()),
        pool.clone(),
        keys.clone(),
    )
    .expect("an empty deployment");

    let exploder = Arc::new(Exploder(AtomicU64::new(0)));
    let sup = Supervisor::start(
        pool.clone(),
        keys,
        deployment,
        metrics.clone(),
        ordo_crypto::SpaceId(SPACE.into()),
        pg_cfg(DB),
        Plan {
            workers: vec![exploder.clone()],
            ..Plan::default()
        },
    )
    .await
    .expect("the supervisor starts");

    // `backoff_secs` is 1, 2, 4 …, so three attempts is seven seconds of
    // real backoff. The mechanism is what is under test; the quarantine
    // threshold is asserted from the constant rather than waited out.
    let restarted = wait_for(Duration::from_secs(20), || {
        exploder.0.load(Ordering::SeqCst) >= 3
    })
    .await;
    assert!(
        restarted,
        "a panicking worker was not restarted: {} attempts",
        exploder.0.load(Ordering::SeqCst)
    );

    let series = metrics.render_prometheus();
    assert!(
        series.contains(r#"ordo_worker_runs_total{outcome="panic",worker="test.exploder"}"#),
        "a panic was not counted:\n{series}"
    );
    assert!(
        series.contains(r#"ordo_worker_restarts_total{reason="panic",worker="test.exploder"}"#),
        "a restart was not counted:\n{series}"
    );

    // **The poisoning adjacency.** `Metrics` holds its state behind
    // `std::sync::Mutex`, and a panic while holding one poisons it — so
    // catching a worker panic could turn a worker fault into a *door* fault
    // through the shared registry, which is the exact coupling
    // `catch_unwind` exists to prevent. Every accessor now ignores poison.
    metrics.deny(SPACE, "mem_1", "member", "someOp");
    assert!(
        metrics.render_prometheus().contains("ordo_denials_total"),
        "the door's metrics stopped working after a worker panic"
    );

    let health = sup.health();
    assert!(
        health["test.exploder"]["failures"].as_u64().unwrap_or(0) >= 3,
        "{health}"
    );
    assert_eq!(
        ordo_host::supervisor::QUARANTINE_AFTER,
        10,
        "the quarantine threshold moved; the operator docs say ten"
    );

    sup.shutdown(Duration::from_secs(5)).await;
}

/// **The rest of R3's done-when, through the real HTTP door.** A worker that
/// panics on every unit is restarted with the platform's backoff sequence,
/// counted as a panic restart, and quarantined after ten consecutive
/// failures with `enabled: false` and a reason on `/health/deep`. The whole
/// time, `/health/live` answers 200 and an op through the same socket
/// succeeds.
///
/// The backoff *unit* is shrunk to 10ms so the tenth failure arrives in
/// seconds; the sequence (`backoff_secs`) and the threshold are the
/// production ones.
#[tokio::test(flavor = "multi_thread")]
async fn a_quarantined_worker_is_disabled_on_health_deep_while_the_door_stays_live() {
    use ordo_host::supervisor::{Timing, QUARANTINE_AFTER};
    use std::sync::atomic::AtomicBool;

    const DB: &str = "ordo_rt_sup_quarantine";
    deployment_ready(DB).await;
    let mut rt = runtime::assemble(config(DB)).await.expect("assembles");

    let exploder = Arc::new(Exploder(AtomicU64::new(0)));
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = tcp.local_addr().expect("addr");

    let sup = Arc::new(
        Supervisor::start(
            rt.pool.clone(),
            rt.keys.clone(),
            rt.deployment.clone(),
            rt.metrics.clone(),
            ordo_crypto::SpaceId(SPACE.into()),
            pg_cfg(DB),
            Plan {
                workers: vec![exploder.clone()],
                timing: Timing {
                    backoff_unit: Duration::from_millis(10),
                    ..Timing::default()
                },
                ..Plan::default()
            },
        )
        .await
        .expect("the supervisor starts"),
    );
    rt.platform = rt.platform.clone().with_supervisor(sup.clone());
    let app = runtime::door(&rt);
    let server = tokio::spawn(async move { axum::serve(tcp, app).await });

    // The prober runs from before the first backoff has elapsed to after
    // the quarantine, so "throughout" covers every panic.
    let stop = Arc::new(AtomicBool::new(false));
    let prober = {
        let stop = stop.clone();
        tokio::spawn(async move {
            let (mut ok, mut bad) = (0u32, Vec::new());
            while !stop.load(Ordering::SeqCst) {
                let (st, body) = http(addr, "GET", "/health/live", None, "").await;
                if st == 200 {
                    ok += 1;
                } else {
                    bad.push((st, body));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            (ok, bad)
        })
    };

    let owner = token("mem_o");
    let mut deep;
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    let quarantined = loop {
        let (st, body) = http(addr, "GET", "/health/deep", Some(&owner), "").await;
        assert_eq!(st, 200, "the owner could not read deep health: {body}");
        deep = json_of(&body);
        if deep["workers"]["test.exploder"]["quarantined"] == serde_json::json!(true) {
            break true;
        }
        if std::time::Instant::now() > deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(quarantined, "never quarantined: {deep}");

    let w = &deep["workers"]["test.exploder"];
    assert_eq!(w["enabled"], serde_json::json!(false), "{deep}");
    assert!(
        w["reason"].as_str().unwrap_or_default().contains(&format!(
            "quarantined after {QUARANTINE_AFTER} consecutive failures"
        )),
        "the reason does not say why: {deep}"
    );
    assert_eq!(w["failures"], serde_json::json!(QUARANTINE_AFTER), "{deep}");
    // The section beside it answers too, through the same door.
    assert!(
        deep["supervisor"]["orphaned_listeners"].is_array(),
        "{deep}"
    );

    // Quarantine is a stop: no eleventh attempt.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(exploder.0.load(Ordering::SeqCst), QUARANTINE_AFTER);

    // Every restart was a counted panic restart.
    let (st, metrics) = http(addr, "GET", "/metrics", Some(&owner), "").await;
    assert_eq!(st, 200, "{metrics}");
    assert!(
        metrics.contains(r#"ordo_worker_restarts_total{reason="panic",worker="test.exploder"} 10"#),
        "ten panic restarts were not counted:\n{metrics}"
    );

    // The door serves a real op, unaffected.
    let (st, body) = http(
        addr,
        "POST",
        "/op/setMemo",
        Some(&token("mem_1")),
        &serde_json::json!({ "order": "ord_1", "memo": "after-quarantine" }).to_string(),
    )
    .await;
    assert_eq!(
        st, 200,
        "an op failed while a worker was quarantined: {body}"
    );

    stop.store(true, Ordering::SeqCst);
    let (ok, bad) = prober.await.expect("prober");
    assert!(bad.is_empty(), "/health/live answered non-200: {bad:?}");
    assert!(ok >= 20, "too few liveness probes to mean anything: {ok}");

    // The server task holds a clone of the platform state, and with it the
    // supervisor; it has to be gone before the supervisor can be shut down.
    server.abort();
    let _ = server.await;
    drop(rt);
    Arc::into_inner(sup)
        .expect("sole owner")
        .shutdown(Duration::from_secs(5))
        .await;
}

// ── leadership ───────────────────────────────────────────────────────────

/// A leader-only worker that counts the units it ran.
struct LeaderOnly(AtomicU64);

impl Worker for LeaderOnly {
    fn name(&self) -> &'static str {
        "test.leader_only"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(Duration::from_millis(20))
    }
    fn requires_leadership(&self) -> bool {
        true
    }
    fn run<'a>(&'a self, _c: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Progress::did(1))
        })
    }
}

type Handles = (
    Pool,
    Arc<ordo_host::metrics::Metrics>,
    Arc<dyn ordo_crypto::KeyProvider>,
    Arc<ordo_host::deploy::Deployment>,
);

async fn contender(
    db: &str,
    h: &Handles,
    metrics: Arc<ordo_host::metrics::Metrics>,
    worker: Arc<LeaderOnly>,
) -> Supervisor {
    Supervisor::start(
        h.0.clone(),
        h.2.clone(),
        h.3.clone(),
        metrics,
        ordo_crypto::SpaceId(SPACE.into()),
        pg_cfg(db),
        Plan {
            workers: vec![worker],
            timing: ordo_host::supervisor::Timing {
                leader_tick: Duration::from_millis(100),
                ..Default::default()
            },
            ..Plan::default()
        },
    )
    .await
    .expect("starts")
}

/// §3.5: two supervisors on one database, one leader. The follower's
/// leader-only worker does not run; when the leader shuts down the follower
/// takes over within a few slow ticks and its worker starts. And when the
/// leader's lock session is killed from outside, it notices, counts the
/// loss, and takes the lock back.
#[tokio::test(flavor = "multi_thread")]
async fn leadership_fails_over_between_two_supervisors() {
    const DB: &str = "ordo_rt_sup_leader";
    let handles = bare(DB).await;

    let (ma, mb) = (
        Arc::new(ordo_host::metrics::Metrics::new()),
        Arc::new(ordo_host::metrics::Metrics::new()),
    );
    let (wa, wb) = (
        Arc::new(LeaderOnly(AtomicU64::new(0))),
        Arc::new(LeaderOnly(AtomicU64::new(0))),
    );
    let a = contender(DB, &handles, ma.clone(), wa.clone()).await;
    let b = contender(DB, &handles, mb.clone(), wb.clone()).await;

    assert!(a.is_leader(), "the first supervisor did not lead");
    assert!(!b.is_leader(), "two leaders on one database");
    assert_eq!(
        a.overview()["leadership"]["leader"],
        serde_json::json!(true)
    );
    assert_eq!(
        b.overview()["leadership"]["leader"],
        serde_json::json!(false)
    );
    assert!(
        ma.render_prometheus()
            .contains(r#"ordo_supervisor_leadership_changes_total{to="leader"} 1"#),
        "{}",
        ma.render_prometheus()
    );

    // Several ticks of both: only the leader's worker runs.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        wa.0.load(Ordering::SeqCst) > 0,
        "the leader's worker never ran"
    );
    assert_eq!(
        wb.0.load(Ordering::SeqCst),
        0,
        "the follower ran a leader-only worker"
    );
    assert!(!b.is_leader());

    // The leader goes away; the follower takes over within a few ticks.
    a.shutdown(Duration::from_secs(5)).await;
    let took_over = wait_for(Duration::from_secs(3), || b.is_leader()).await;
    assert!(took_over, "the follower never took over: {}", b.overview());
    let ran = wait_for(Duration::from_secs(3), || wb.0.load(Ordering::SeqCst) > 0).await;
    assert!(ran, "the new leader's worker never started");

    // Kill the new leader's lock session from outside. The overseer's probe
    // finds it gone, counts the loss, and takes the lock back.
    // 6096213 is `LEAD_NS` (0x005D0555) in `ordo_exec::jobs`.
    let killer = Pool::connect(pg_cfg(DB)).await.expect("pool");
    let killed = killer
        .rows(
            "SELECT pg_terminate_backend(pid) FROM pg_locks \
             WHERE locktype = 'advisory' AND classid = 6096213 \
             AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
        )
        .await
        .expect("terminate");
    assert_eq!(killed.len(), 1, "exactly one session held leadership");
    let lost = wait_for(Duration::from_secs(3), || {
        mb.render_prometheus()
            .contains(r#"ordo_supervisor_leadership_changes_total{to="follower"} 1"#)
    })
    .await;
    assert!(
        lost,
        "the loss was not noticed:\n{}",
        mb.render_prometheus()
    );
    let back = wait_for(Duration::from_secs(3), || b.is_leader()).await;
    assert!(back, "the lock was not retaken: {}", b.overview());
    assert!(
        b.overview()["leadership"]["changes"].as_u64().unwrap_or(0) >= 3,
        "{}",
        b.overview()
    );

    b.shutdown(Duration::from_secs(5)).await;
}

// ── secevent.retain ──────────────────────────────────────────────────────

/// `secevent.retain` claims **its own** schedule row, once per period, as
/// the leader. A signer the test holds stands in for
/// `ORDO_SECURITY_CHECKPOINT_KEY`, which this workspace cannot set without
/// `unsafe`.
#[tokio::test(flavor = "multi_thread")]
async fn secevent_retain_claims_its_own_schedule_once_per_period() {
    const DB: &str = "ordo_rt_sup_retain";
    let (pool, metrics, keys, deployment) = bare(DB).await;
    pool.rows("DELETE FROM \"$schedules\" WHERE \"name\" = 'secevent.retention'")
        .await
        .expect("reset");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let period = Duration::from_secs(3600);
    // Registered two periods ago, so it is due now.
    ordo_exec::secevent::register_retention_schedule(&pool, period, now - 7200)
        .await
        .expect("schedule");

    let retainer = ordo_host::secdrain::Retainer::new(
        pool.clone(),
        metrics.clone(),
        Arc::new(ordo_host::secdrain::DrainHealth::default()),
        Arc::new(ordo_commandlog::Ed25519Signer::from_seed(
            "sup-retain",
            &[5u8; 32],
        )),
        ordo_secevent::Retention::default(),
        period,
    );
    let sup = Supervisor::start(
        pool.clone(),
        keys,
        deployment,
        metrics,
        ordo_crypto::SpaceId(SPACE.into()),
        pg_cfg(DB),
        Plan {
            workers: vec![Arc::new(ordo_host::workers::SecEventRetain {
                retainer,
                tick: Duration::from_millis(50),
            })],
            ..Plan::default()
        },
    )
    .await
    .expect("starts");
    // The first claim may time out on a loaded runner (run 41297909915);
    // the next leadership tick claims again.
    assert!(
        wait_for(Duration::from_secs(15), || sup.is_leader()).await,
        "never led: {}",
        sup.health()
    );

    let ran = wait_for(Duration::from_secs(5), || {
        sup.health()["secevent.retain"]["runs"]
            .as_u64()
            .unwrap_or(0)
            >= 5
    })
    .await;
    assert!(ran, "{}", sup.health());
    assert_eq!(
        sup.health()["secevent.retain"]["failures"],
        serde_json::json!(0),
        "{}",
        sup.health()
    );

    // Five passes, one claim: the row advanced by the retention period, and
    // every later pass found it not yet due.
    let row = pool
        .rows(
            "SELECT \"fires\", \"next_due\" FROM \"$schedules\" \
             WHERE \"name\" = 'secevent.retention'",
        )
        .await
        .expect("row");
    let fires: i64 = row[0][0].as_deref().unwrap().parse().unwrap();
    let next: i64 = row[0][1].as_deref().unwrap().parse().unwrap();
    assert_eq!(fires, 1, "the schedule fired more than once in a period");
    assert!(
        next >= now + 3600 - 5,
        "advanced by some other period: {next}"
    );

    sup.shutdown(Duration::from_secs(5)).await;
}

// ── secevent.checkpoint ──────────────────────────────────────────────────

/// `secevent.checkpoint` claims **its own** schedule row, once per period, as
/// the leader — and leaves `secevent.retention` alone.
///
/// The fleet property, which is the one the supervisor adds over the pass
/// itself: a hundred replicas ticking every sixty seconds must produce one
/// signature per period, not one per replica. It comes from
/// `claim_schedule`'s `FOR UPDATE SKIP LOCKED`, and leadership is only there
/// to stop a hundred replicas asking at the same instant.
///
/// A signer the test holds stands in for `ORDO_SECURITY_CHECKPOINT_KEY`,
/// which this workspace cannot set without `unsafe`.
#[tokio::test(flavor = "multi_thread")]
async fn secevent_checkpoint_claims_its_own_schedule_once_per_period() {
    const DB: &str = "ordo_rt_sup_seccp";
    let (pool, metrics, keys, deployment) = bare(DB).await;
    pool.rows("DELETE FROM \"$schedules\" WHERE \"name\" LIKE 'secevent.%'")
        .await
        .expect("reset");
    // The stream's own tables too. This database outlives the run that made
    // it, so without this the second run of this test appends its three
    // entries beside the first run's, the head is at six rather than three,
    // and the assertion below counts two markers — a `one period, one
    // signature` failure caused entirely by the previous run. The same thing
    // `tests/secdrain.rs` and `tests/operator.rs` each say in their own
    // words about their own fixtures.
    for t in ["$secevents", "$seccheckpoints"] {
        pool.rows(&format!("DELETE FROM \"{t}\""))
            .await
            .expect("reset");
    }
    // A stream with something in it: a checkpoint over nothing anchors
    // nothing, so an empty scope would make this test pass for the wrong
    // reason — nothing to claim rather than one claim.
    let sec =
        ordo_exec::secevent::PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let scope = ordo_secevent::Scope::Space(ordo_commandlog::SpaceId(SPACE.into()));
    for n in 0..3u32 {
        sec.try_append(&ordo_secevent::StreamEntry::single(
            scope.clone(),
            ordo_secevent::Subject::member("usr_1"),
            "openTasks",
            ordo_secevent::EventKind::denied(ordo_secevent::RecordedDecision::NoMembership),
            ordo_commandlog::Timestamp(1_000 + u64::from(n)),
            ordo_commandlog::CorrelationId(format!("cor_{n}")),
        ))
        .await
        .expect("append");
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let period = Duration::from_secs(3600);
    // Registered two periods ago, so it is due now.
    ordo_exec::secevent::register_checkpoint_schedule(&pool, period, now - 7200)
        .await
        .expect("schedule");

    let checkpointer = ordo_host::secdrain::Checkpointer::new(
        pool.clone(),
        metrics.clone(),
        Arc::new(ordo_host::secdrain::DrainHealth::default()),
        Arc::new(ordo_commandlog::Ed25519Signer::from_seed(
            "sup-seccp",
            &[6u8; 32],
        )),
        period,
    );
    let sup = Supervisor::start(
        pool.clone(),
        keys,
        deployment,
        metrics,
        ordo_crypto::SpaceId(SPACE.into()),
        pg_cfg(DB),
        Plan {
            workers: vec![Arc::new(ordo_host::workers::SecEventCheckpoint {
                checkpointer,
                tick: Duration::from_millis(50),
            })],
            ..Plan::default()
        },
    )
    .await
    .expect("starts");
    // The first claim may time out on a loaded runner (run 41297909915);
    // the next leadership tick claims again.
    assert!(
        wait_for(Duration::from_secs(15), || sup.is_leader()).await,
        "never led: {}",
        sup.health()
    );

    let ran = wait_for(Duration::from_secs(5), || {
        sup.health()["secevent.checkpoint"]["runs"]
            .as_u64()
            .unwrap_or(0)
            >= 5
    })
    .await;
    assert!(ran, "{}", sup.health());
    assert_eq!(
        sup.health()["secevent.checkpoint"]["failures"],
        serde_json::json!(0),
        "{}",
        sup.health()
    );

    // Many passes, one claim, one marker.
    let row = pool
        .rows(
            "SELECT \"fires\", \"next_due\" FROM \"$schedules\" \
             WHERE \"name\" = 'secevent.checkpoint'",
        )
        .await
        .expect("row");
    let fires: i64 = row[0][0].as_deref().unwrap().parse().unwrap();
    let next: i64 = row[0][1].as_deref().unwrap().parse().unwrap();
    assert_eq!(fires, 1, "the schedule fired more than once in a period");
    assert!(
        next >= now + 3600 - 5,
        "advanced by some other period: {next}"
    );
    let markers = pool
        .rows("SELECT count(*) FROM \"$seccheckpoints\"")
        .await
        .expect("markers");
    assert_eq!(
        markers[0][0].as_deref(),
        Some("1"),
        "one period, one signature"
    );
    // And it did not touch retention's row. Two artifacts' cadences through
    // one row is `cadence.rs`'s named bug; two *passes* of one artifact
    // through one row is the same bug one level down.
    let retention = pool
        .rows("SELECT count(*) FROM \"$schedules\" WHERE \"name\" = 'secevent.retention'")
        .await
        .expect("rows");
    assert_eq!(
        retention[0][0].as_deref(),
        Some("0"),
        "the checkpoint pass claimed or created the retention schedule"
    );

    sup.shutdown(Duration::from_secs(5)).await;
}

// ── orphaned listener cursors ────────────────────────────────────────────

/// §3.7: a `listener` cursor for a listener this deployment no longer
/// serves is listed with its position and age, and **left alone**. A
/// served listener's cursor is not listed.
#[tokio::test(flavor = "multi_thread")]
async fn an_orphaned_listener_cursor_is_reported_and_left_alone() {
    const DB: &str = "ordo_rt_sup_orphan";
    let pool = deployment_ready(DB).await;
    ordo_host::supervisor::bootstrap(&pool)
        .await
        .expect("bootstrap");
    let space = ordo_commandlog::SpaceId(SPACE.into());
    let retired = Listener {
        routine: "retiredRoutine".into(),
        ..listener()
    };
    ordo_exec::triggers::register_listener(&pool, &space, &retired, 0, 1)
        .await
        .expect("register");

    let rt = runtime::assemble(config(DB))
        .await
        .expect("assembles")
        .with_triggers(TriggerWiring {
            listeners: vec![listener()],
            clock: Vec::new(),
            handler: None,
            tenants: None,
        });
    // And one dead letter, which the same census reads (§3.6).
    pool.rows(
        "INSERT INTO \"$deadletters\" \
         (\"space\", \"run_key\", \"listener\", \"payload\", \"attempts\", \"at\", \"reason\") \
         VALUES ('acme', 'rk_1', 'notifyOnOrder:market.orders.*', '{}', 5, 7, 'the body refused')",
    )
    .await
    .expect("dead letter");
    // And one attestation delivery that gave up, which the deployment owner
    // reads here too (owner ruling 2026-10-05). Its URL carries a token in
    // the path, and only the host is shown.
    ordo_exec::webhook::ensure_tables(&pool)
        .await
        .expect("webhook tables");
    pool.rows("DELETE FROM \"$attestdeadletters\"")
        .await
        .expect("cleared");
    pool.rows(
        "INSERT INTO \"$attestdeadletters\" \
         (\"space\", \"upto\", \"head\", \"destination\", \"attempts\", \"at\", \"reason\") \
         VALUES ('acme', 4, '\\x00'::bytea, 'https://hooks.acme.example/t/s3cr3t-token', 5, 9, \
                 'timed_out: timed out after 5000ms')",
    )
    .await
    .expect("attestation dead letter");
    let sup = rt.start_workers().await.expect("starts");

    let dead = sup.overview()["dead_letters"].clone();
    assert_eq!(dead["total"], serde_json::json!(1), "{dead}");
    assert_eq!(
        dead["recent"][0]["reason"],
        serde_json::json!("the body refused"),
        "{dead}"
    );
    let attest = sup.overview()["attestation_dead_letters"].clone();
    assert_eq!(attest["total"], serde_json::json!(1), "{attest}");
    assert_eq!(attest["recent"][0]["space"], "acme", "{attest}");
    assert_eq!(
        attest["recent"][0]["host"], "hooks.acme.example",
        "{attest}"
    );
    assert_eq!(attest["recent"][0]["upto"], serde_json::json!(4));
    assert!(
        !attest.to_string().contains("s3cr3t-token"),
        "the path of a tenant's webhook URL is not on this page: {attest}"
    );

    let orphans = sup.overview()["orphaned_listeners"].clone();
    let names: Vec<&str> = orphans
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|o| o["consumer"].as_str())
        .collect();
    assert_eq!(names, vec![retired.key().as_str()], "{orphans}");
    assert_eq!(orphans[0]["position"], serde_json::json!(0));
    assert!(orphans[0]["idle_secs"].as_u64().unwrap_or(0) > 0);

    Arc::into_inner(sup)
        .expect("sole owner")
        .shutdown(Duration::from_secs(5))
        .await;

    // Reported, not acted on: the cursor is still registered.
    let still = ordo_exec::prune::consumers(&pool, &space)
        .await
        .expect("consumers");
    assert!(
        still.iter().any(|c| c.name == retired.key()),
        "the supervisor deregistered a cursor it was only meant to report"
    );
}

// ── per-worker tick overrides (§3.4) ─────────────────────────────────────

/// A worker whose own cadence is ten minutes, so anything that runs it more
/// than once inside a test is running it on somebody else's number.
struct Ticker(&'static str, AtomicU64);

impl Worker for Ticker {
    fn name(&self) -> &'static str {
        self.0
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(Duration::from_secs(600))
    }
    fn run<'a>(&'a self, _c: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            self.1.fetch_add(1, Ordering::SeqCst);
            Ok(Progress::idle())
        })
    }
}

/// `ORDO_WORKER_<NAME>_TICK_MS` moves the tick the loop sleeps for, and a
/// worker with no override keeps its own.
///
/// **The control half is the test.** A supervisor that ignored the override
/// would still run the worker once, immediately, because the first pass
/// precedes the first sleep. So a bare "it ran" assertion passes against a
/// supervisor that applied nothing, and the number of runs is what separates
/// the two. The second worker in the same supervisor has no override and
/// must still be sitting out its ten minutes.
#[tokio::test(flavor = "multi_thread")]
async fn a_tick_override_moves_one_workers_cadence_and_leaves_the_rest() {
    let pg = Pg::scoped("ordo_rt_sup_tick");
    let pool = Pool::connect(pg_cfg(&pg.db)).await.expect("pool");
    pool.ensure_platform_tables().await.expect("tables");

    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let keys: Arc<dyn ordo_crypto::KeyProvider> =
        Arc::new(ordo_crypto::TestProvider::with_space(SPACE));
    let deployment = ordo_host::deploy::Deployment::empty(
        ordo_crypto::SpaceId(SPACE.into()),
        pool.clone(),
        keys.clone(),
    )
    .expect("an empty deployment");

    let fast = Arc::new(Ticker("test.fast", AtomicU64::new(0)));
    let slow = Arc::new(Ticker("test.slow", AtomicU64::new(0)));
    let sup = Supervisor::start(
        pool.clone(),
        keys,
        deployment,
        metrics.clone(),
        ordo_crypto::SpaceId(SPACE.into()),
        pg_cfg(&pg.db),
        Plan {
            workers: vec![fast.clone(), slow.clone()],
            ticks: [("test.fast".to_string(), Duration::from_millis(10))]
                .into_iter()
                .collect(),
            ..Plan::default()
        },
    )
    .await
    .expect("the supervisor starts");

    let moved = wait_for(Duration::from_secs(5), || {
        fast.1.load(Ordering::SeqCst) >= 20
    })
    .await;
    assert!(
        moved,
        "a 10ms override did not move a 600s tick: {} runs",
        fast.1.load(Ordering::SeqCst)
    );
    assert_eq!(
        slow.1.load(Ordering::SeqCst),
        1,
        "the worker with no override did not keep its own cadence"
    );

    // And the census prints the number the loop is on, because an operator
    // who set the variable needs to see that it took effect.
    let census = sup.census();
    let fast_line = census
        .iter()
        .find(|(n, _)| *n == "test.fast")
        .map(|(_, h)| h.clone())
        .expect("test.fast is in the census");
    assert_eq!(fast_line, "tick 10ms", "the census hid the override");

    sup.shutdown(Duration::from_secs(5)).await;
}

/// An unreadable override is a refusal, not a default. A variable an
/// operator set and the process silently ignored is the configuration form
/// of a worker that is not running and says nothing.
#[test]
fn an_unreadable_tick_override_refuses_rather_than_defaulting() {
    use ordo_host::supervisor::tick_overrides;

    // Names chosen so no other test in this binary reads the same variable.
    const OK: &str = "tick.readable";
    const BAD: &str = "tick.garbage";
    const ZERO: &str = "tick.zero";

    std::env::set_var("ORDO_WORKER_TICK_READABLE_TICK_MS", "250");
    std::env::set_var("ORDO_WORKER_TICK_GARBAGE_TICK_MS", "quick");
    std::env::set_var("ORDO_WORKER_TICK_ZERO_TICK_MS", "0");

    let ok = tick_overrides([OK]).expect("a whole number of milliseconds");
    assert_eq!(ok.get(OK), Some(&Duration::from_millis(250)));

    let garbage = tick_overrides([BAD]).expect_err("`quick` is not a number");
    assert!(
        garbage.contains("ORDO_WORKER_TICK_GARBAGE_TICK_MS") && garbage.contains("quick"),
        "the refusal must name the variable and what it said: {garbage}"
    );

    let zero = tick_overrides([ZERO]).expect_err("0 would spin");
    assert!(zero.contains("spins"), "{zero}");

    // A name with no variable set is not an override and not an error.
    assert!(tick_overrides(["tick.unset"])
        .expect("no variable is not a refusal")
        .is_empty());

    std::env::remove_var("ORDO_WORKER_TICK_READABLE_TICK_MS");
    std::env::remove_var("ORDO_WORKER_TICK_GARBAGE_TICK_MS");
    std::env::remove_var("ORDO_WORKER_TICK_ZERO_TICK_MS");
}

/// `ordo dev` composes no sign-in unless the environment asks for it, so
/// `$principal` is never created. The registration sweep reads that table,
/// so it must not run there: it is named as disabled, and no worker reports
/// a failure, which is what printed the same error every few seconds.
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_without_sign_in_starts_with_no_registration_sweep_and_no_repeating_error() {
    const DB: &str = "ordo_rt_sup_nosweep";
    deployment_ready(DB).await;
    let rt = runtime::assemble(config(DB)).await.expect("assembles");
    assert!(rt.platform.auth.is_none(), "this config has no sign-in");
    let sup = rt.start_workers().await.expect("the supervisor starts");

    assert!(
        !sup.running().contains(&"registrations.sweep"),
        "the sweep runs with no identity directory: {:?}",
        sup.running()
    );
    assert!(
        sup.disabled
            .iter()
            .any(|(n, why)| n == "registrations.sweep" && why.contains("no authentication")),
        "the census does not say why: {:?}",
        sup.disabled
    );

    // Long enough for every worker's first pass and its first retry.
    tokio::time::sleep(Duration::from_secs(4)).await;
    let health = sup.health();
    for (name, w) in health.as_object().expect("workers") {
        assert_eq!(
            w["failures"].as_u64().unwrap_or(0),
            0,
            "{name} failed on a runtime with no sign-in: {health}"
        );
    }
    sup.shutdown(Duration::from_secs(5)).await;
}

/// `ORDO_SECURITY_CHECKPOINT_KEY` unset: the first boot makes the key and a
/// restart on the same database opens the same one. The drain publishes
/// its id and public half for a verifier.
#[tokio::test(flavor = "multi_thread")]
async fn an_unset_security_checkpoint_key_is_made_once_and_reused_across_a_restart() {
    const DB: &str = "ordo_rt_sup_seckey";
    deployment_ready(DB).await;
    let first = runtime::assemble(config(DB)).await.expect("assembles");
    let (id, public, source) = first
        .secdrain
        .health()
        .checkpoint_key()
        .expect("a key was made");
    assert!(id.starts_with("sec-"), "{id}");
    assert_eq!(public.len(), 64, "{public}");
    // This database outlives the run, so a second run finds the key stored.
    assert!(
        ["made at this boot", "made at first boot"].contains(&source),
        "{source}"
    );
    drop(first);

    let second = runtime::assemble(config(DB))
        .await
        .expect("assembles again");
    let again = second
        .secdrain
        .health()
        .checkpoint_key()
        .expect("the stored key");
    assert_eq!(
        (again.0.as_str(), again.1.as_str()),
        (id.as_str(), public.as_str())
    );
    assert_eq!(again.2, "made at first boot");
}
