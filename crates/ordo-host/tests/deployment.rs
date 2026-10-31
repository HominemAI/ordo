//! 8n / 8o / 8p / 9j — the machinery that now reaches somebody.
//!
//! One theme: a load gate that can fail, a deployment surface that is
//! actually mounted, a counter substrate the mandatory alerts can read, and
//! an invalidation path that crosses nodes so `?live=1` can mean something.

use std::sync::Arc;

use ordo_conformance::{Fixture, Pg};
use ordo_exec::PoolConfig;
use ordo_host::shape::{verify_registry, ShapeRefusal};

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        ..PoolConfig::from_env()
    }
}

fn make_db(db: &str) -> Pg {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    Pg {
        db: db.to_string(),
        ..Pg::default()
    }
}

// ── 8n: the load gate ────────────────────────────────────────────────────

/// The gate covers the registry, and says exactly what it covered.
#[tokio::test(flavor = "multi_thread")]
async fn the_load_gate_verifies_the_registry_and_reports_its_coverage() {
    let f = Fixture::load().expect("fixture");
    let pg = make_db("ordo_host_gate");
    f.reset(&pg).expect("seeded");

    let report = verify_registry(&f, &cfg("ordo_host_gate"), &f.ddl)
        .await
        .expect("a registry that agrees with its database boots");

    // Every registered op is accounted for in exactly one bucket.
    assert_eq!(
        report.total(),
        f.registry.names().count(),
        "an op fell out of the report entirely: {}",
        report.summary()
    );
    // Placeholder binding is what makes the gate cover the registry rather
    // than the seven ops that happen to take no arguments — 26 of the
    // fixture's 27 reads, against the 7 the mechanism's own test could
    // reach. If this number collapses, the gate has gone hollow without
    // failing.
    assert!(
        report.verified.len() >= 25,
        "only {} ops verified — {}\nundescribable: {:?}",
        report.verified.len(),
        report.summary(),
        report.undescribable
    );
    // And the skips stay a short, named list rather than a growing silence.
    assert!(
        report.undescribable.len() <= 2,
        "the gate is skipping too much: {:?}",
        report.undescribable
    );
    // Parameterized reads are verified, not skipped.
    assert!(
        report.verified.iter().any(|o| o == "cheapActive"),
        "a parameterized read should be verifiable: {:?}",
        report.undescribable
    );
    // Writes have no result shape, and the report says so rather than
    // counting them as passes.
    assert!(report.no_result_shape.iter().any(|o| o == "addTask"));
    // Every skip carries a reason a reader can weigh.
    for u in &report.undescribable {
        assert!(!u.reason.is_empty(), "`{}` was skipped silently", u.op);
    }
    println!("{}", report.summary());
}

/// The gate has to BITE. A registry whose inferred shape disagrees with the
/// database must refuse to serve, naming the op and both shapes.
#[tokio::test(flavor = "multi_thread")]
async fn a_registry_that_disagrees_with_its_database_refuses_to_serve() {
    let f = Fixture::load().expect("fixture");
    let pg = make_db("ordo_host_gate_drift");
    f.reset(&pg).expect("seeded");

    // Schema drift, the realistic way it happens: a migration changed a
    // column's type and the compiled registry still believes the old one.
    // `tasks.title` is `text` to the compiler; the database now says `date`.
    let drifted = f
        .ddl
        .replace("\"title\" text NOT NULL", "\"title\" date NOT NULL");
    assert_ne!(drifted, f.ddl, "the DDL edit found nothing to change");

    let refusal = verify_registry(&f, &cfg("ordo_host_gate_drift"), &drifted)
        .await
        .expect_err("a divergent registry must not boot");

    let ShapeRefusal::Mismatch(mismatches) = &refusal else {
        panic!("expected a shape mismatch, got {refusal}");
    };
    let text = refusal.to_string();
    // It names the ops that diverged…
    assert!(
        mismatches.iter().any(|m| m.op == "openTasks"),
        "the refusal does not name the op that diverged: {text}"
    );
    // …and says what the disagreement was. Two shapes of divergence reach
    // here and BOTH must refuse: the database describes the plan and
    // reports a different type, or it refuses to describe the plan at all.
    // The second is not a lesser case — a schema that cannot run the
    // statement is more divergent, not less — and it is the shape a
    // collation pin produced the day one landed, when the gate had been
    // filing it under "undescribable" and passing.
    assert!(
        text.contains("the database reports `date`")
            || text.contains("the database refused to describe"),
        "the refusal must say what disagreed: {text}"
    );
    // And it is a refusal to serve, not a warning: there is no ShapeReport,
    // so `PlatformState::new` cannot be called and no router exists.
}

/// Board `guard-equality-uses-no-index`: `==` compiles to a plain `=` where
/// the database's `NOT NULL` proves a side never null. A live database that
/// has lost one of those constraints, or a required ref's foreign key, is
/// refused at load, and the refusal names the column.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_that_lost_a_not_null_the_program_relies_on_refuses_to_serve() {
    let f = Fixture::load().expect("fixture");
    let pg = make_db("ordo_host_gate_not_null");
    f.reset(&pg).expect("seeded");
    verify_registry(&f, &cfg("ordo_host_gate_not_null"), &f.ddl)
        .await
        .expect("the database as the DDL made it serves");

    pg.exec(
        "ALTER TABLE \"projectMembers\" ALTER COLUMN \"member\" DROP NOT NULL; \
         ALTER TABLE \"tasks\" DROP CONSTRAINT \"tasks$project$fk\";",
    )
    .expect("the constraints are dropped behind the program's back");
    let refusal = verify_registry(&f, &cfg("ordo_host_gate_not_null"), &f.ddl)
        .await
        .expect_err("a lost NOT NULL must not boot");
    let ShapeRefusal::NeverNullGone(lines) = &refusal else {
        panic!("expected the never-null refusal, got {refusal}");
    };
    let text = refusal.to_string();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("`projectMembers`.`member` allows nulls")),
        "the refusal names the column: {text}"
    );
    assert!(
        lines.iter().any(|l| l.contains("`tasks$project$fk`")),
        "the refusal names the foreign key: {text}"
    );
}

/// A gate whose coverage collapsed to zero is not a gate, and says so
/// rather than passing.
#[tokio::test(flavor = "multi_thread")]
async fn a_gate_that_verifies_nothing_refuses() {
    let f = Fixture::load().expect("fixture");
    let pg = make_db("ordo_host_gate_empty");
    f.reset(&pg).expect("seeded");

    // A shadow with no tables in it. Every op's plan is refused by the
    // database — `relation "products" does not exist` — which is the most
    // divergent a schema can be, so the gate refuses.
    //
    // It reports Mismatch rather than NoCoverage, and that is the honest
    // classification: the database did not decline to answer, it answered
    // that it cannot run these statements. `NoCoverage` remains for the
    // case it names — a registry whose reads are all undescribable for
    // reasons that never reach the database at all.
    let refusal = verify_registry(&f, &cfg("ordo_host_gate_empty"), "SELECT 1")
        .await
        .expect_err("zero coverage is not a pass");
    let text = refusal.to_string();
    assert!(
        text.contains("does not exist"),
        "the refusal must carry the database's own reason: {text}"
    );
    assert!(
        matches!(
            refusal,
            ShapeRefusal::Mismatch(_) | ShapeRefusal::NoCoverage(_)
        ),
        "either refusal is correct here; passing is not: {text}"
    );
}

// ── shared deployment assembly ───────────────────────────────────────────

/// One node: its own pool, its own executor, its own bus, and its own
/// security-event drain.
struct Node {
    api: ordo_api::ApiState,
    platform: ordo_host::serve::PlatformState,
    /// Held for its lifetime, exactly like the invalidation listener:
    /// dropping it stops the drain task. A node that assembled one and let
    /// it fall out of scope would be a node whose security stream silently
    /// stopped reaching the database.
    _secdrain: Option<ordo_host::secdrain::SecurityDrain>,
}

impl Node {
    fn router(&self) -> axum::Router {
        ordo_api::router(self.api.clone())
            .merge(ordo_host::serve::platform_router(self.platform.clone()))
    }
}

/// A fixture whose key custody is **metered**. Wrapping at the fixture is
/// what puts the counter on both halves of the platform: the executor reads
/// through `f.keys`, and the Stage-5 write protocol seals through the very
/// same handle.
fn metered_fixture(metrics: &Arc<ordo_host::metrics::Metrics>) -> Arc<Fixture> {
    let mut f = Fixture::load().expect("fixture");
    f.keys = Arc::new(ordo_host::metrics::MeteredKeys::new(
        f.keys.clone(),
        metrics.clone(),
    ));
    Arc::new(f)
}

/// Boot a node against `db`, with the shape gate in front of it. `cross`
/// selects the cross-node bus (Postgres LISTEN) over the in-process one.
/// `stream`, when given, is the recorder the node's door writes denials
/// into — so the drain this boots is draining the real one rather than a
/// second recorder nothing feeds.
async fn boot(
    f: &Arc<Fixture>,
    pg: &Pg,
    db: &str,
    cross: bool,
    metrics: Arc<ordo_host::metrics::Metrics>,
    stream: Option<Arc<ordo_secevent::Recorder>>,
) -> Node {
    // 8n: the load path that can fail, ahead of everything else.
    let report = verify_registry(f, &cfg(db), &f.ddl)
        .await
        .expect("the registry's shapes agree with the database");

    let pool = ordo_exec::Pool::connect(cfg(db)).await.expect("pool boots");
    // 5f: the handle the write path uses reports the chain-append lock's
    // wait and its critical-section width to this node's registry. A
    // separate handle over the same connections, so the reporting is this
    // deployment's choice rather than a property of the pool.
    let pool = pool.observing(metrics.clone());
    // Custody is metered at the fixture (see `metered_fixture`), so both
    // the read path and the write protocol share one counted handle.
    let keys: Arc<dyn ordo_crypto::KeyProvider> = f.keys.clone();
    // One `Loaded` version behind one lock: the specs the door dispatches
    // on, the fixture the executor plans against, the manifests the catalog
    // renders, and the report that vouched for all three. `from_gate` takes
    // the report by value, so this harness cannot assemble a deployment the
    // gate did not pass either.
    let deployment = ordo_host::deploy::Deployment::boot(
        ordo_host::deploy::Verified::from_gate(
            "deployment-test",
            f.clone(),
            ordo_exec::Executor {
                pool: pool.clone(),
                schema: f.schema.clone(),
                registry: f.registry.clone(),
                keys: keys.clone(),
                space: ordo_conformance::fixture_space(),
            },
            report,
        )
        .expect("a gated fixture is installable"),
    );
    boot_on(deployment, pg, db, pool, cross, metrics, stream).await
}

/// The half of [`boot`] that assembles a door over a deployment somebody
/// else gated.
///
/// Split out for R2a: a node booted on a `Verified` that came out of
/// `deploy::verify_candidate` is the only way to test the *load* path's
/// custody, and re-deriving the fixture and the provider from the
/// deployment rather than taking them as arguments is what makes it a
/// test of that path rather than of the arguments the test passed.
async fn boot_on(
    deployment: Arc<ordo_host::deploy::Deployment>,
    pg: &Pg,
    db: &str,
    pool: ordo_exec::Pool,
    cross: bool,
    metrics: Arc<ordo_host::metrics::Metrics>,
    stream: Option<Arc<ordo_secevent::Recorder>>,
) -> Node {
    use ordo_api::{ApiState, InvalidationBus, MemoryBus, MemoryReplayCache, PageKey};

    // The live program's own fixture and its own custody — not a second
    // pair the caller happened to hold. If loading downgraded custody, this
    // door serves the downgrade.
    let f = deployment.current().fixture.clone();
    let keys: Arc<dyn ordo_crypto::KeyProvider> = f.keys.clone();
    let registry = deployment.handle();
    let bus: Arc<dyn InvalidationBus> = if cross {
        Arc::new(
            ordo_host::invalidation::PgInvalidationBus::connect(
                &f,
                &cfg(db),
                Some(metrics.clone()),
            )
            .await
            .expect("listener connects"),
        )
    } else {
        Arc::new(MemoryBus::new())
    };
    let api = ApiState {
        registry: registry.clone(),
        // 5f: wrapped, so `ordo_op_duration_seconds{op,outcome}` — the p99
        // measurement's prerequisite — is recorded for every dispatch. A
        // decorator, so a deployment that wants no metering does not wrap.
        executor: Arc::new(ordo_host::metrics::MeteredExecutor::new(
            Arc::new(ordo_host::FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: pg.clone(),
                page_size: 50,
            }),
            metrics.clone(),
        )),
        access: Arc::new(ordo_host::MemberAccess {
            fixture: f.clone(),
            pg: pg.clone(),
        }),
        quota: Arc::new(ordo_api::NoQuota),
        bus,
        replay: Arc::new(MemoryReplayCache::new()),
        // The real one, not `NoAttestation`: this is the suite with a pool
        // and a chain behind it, so layer-1 pinning is exercised on every
        // response it makes rather than only in its own test.
        attest: Arc::new(ordo_host::attest::PgAttestation::new(
            pool.clone(),
            Box::new(ordo_commandlog::sign::Ed25519Signer::from_seed(
                "deployment-test",
                &[7u8; 32],
            )),
        )),
        page_key: PageKey::from_secret(b"ordo-deployment-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    // 7c: the security-event drain, on the same shape as the invalidation
    // listener above — a background task over the deployment's own pool,
    // held by the node for its lifetime. Without it the recorder the door
    // writes into reaches no database at all, and a restart loses every
    // unwritten denial. `stream` is the door's own recorder, so this is
    // the real path and not a second one built for the test.
    let recorder = stream.unwrap_or_default();
    let secdrain = ordo_host::secdrain::SecurityDrain::start(
        recorder,
        pool.clone(),
        metrics.clone(),
        ordo_host::secdrain::DrainConfig {
            tick: std::time::Duration::from_millis(50),
            // Retention is left ON — the deployment's default — precisely
            // so this harness exercises what a deployment that has not set
            // `ORDO_SECURITY_CHECKPOINT_KEY` actually gets: retention
            // reports itself INACTIVE and prunes nothing, rather than
            // cutting at a position no signature vouches for.
            ..Default::default()
        },
    )
    .await
    .expect("the security drain starts");

    let platform = ordo_host::serve::PlatformState::new(
        api.access.clone(),
        deployment,
        pool,
        keys,
        ordo_host::DEFAULT_SPACE,
        metrics,
    )
    .with_security_drain(secdrain.health());
    Node {
        api,
        platform,
        _secdrain: Some(secdrain),
    }
}

async fn get(router: axum::Router, path: &str, token: Option<&str>) -> (u16, String) {
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    let mut req = Request::builder()
        .method("GET")
        .uri(path)
        .header("x-ordo-space", ordo_host::DEFAULT_SPACE);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = router
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

// ── R2a: custody is an input to loading a program ────────────────────────

/// **R2a's done-when.** A program loaded through `verify_candidate` seals
/// through the provider the operator supplied — and the ciphertext a write
/// leaves in the database does *not* open under `TestProvider`'s constant.
///
/// This is the defect the runtime plan reordered itself around. Custody
/// reached the `Executor`, which covers reads; the Stage-5 write protocol
/// seals through `Fixture::keys`, which `compile_source` hardcoded to
/// `TestProvider::with_space(FIXTURE_SPACE)`. So a correctly configured
/// deployment read through the operator's custody and wrote under a master
/// key published in this repository — a working round trip whose ciphertext
/// anyone with a clone could open.
///
/// Both halves are asserted, because either alone is passable by accident:
/// identity of the provider (`Arc::ptr_eq`, never a type name — a second,
/// differently-configured `LocalKeyProvider` is exactly the bug), and the
/// bytes on disk.
#[tokio::test(flavor = "multi_thread")]
async fn a_loaded_program_seals_under_the_operators_custody() {
    use axum::body::Body;
    use axum::http::Request;
    use ordo_crypto::{KeyProvider, Purpose};
    use tower::util::ServiceExt;

    const DB: &str = "ordo_host_r2a_custody";
    let space = ordo_conformance::fixture_space();

    // The operator's custody: a master key that is not in this repository,
    // over a store of its own. `create_space` is deliberate — a provider
    // that minted custody on demand would be `TestProvider`'s convenience,
    // and §2.5 refuses it.
    let local = ordo_crypto::LocalKeyProvider::new(
        b"an-operator-master-key-that-is-not-in-this-repository",
        Box::new(ordo_crypto::MemoryKeyStore::new()),
    );
    local.create_space(&space).expect("the operator provisions");
    let operator: Arc<dyn KeyProvider> = Arc::new(local);

    // Seed through the SAME custody the deployment will hold: the seed's
    // blind-index digests are `HMAC(BIK(space), value)` and BIK is
    // SMK-derived, so a database seeded under one provider and served
    // under another has digests nothing can address.
    let mut seeder = Fixture::load().expect("fixture");
    seeder.keys = operator.clone();
    let pg = make_db(DB);
    seeder.reset(&pg).expect("seeded");

    let pool = ordo_exec::Pool::connect(cfg(DB)).await.expect("pool boots");

    // The load path, with the operator's provider as an argument — exactly
    // what `deploy::load_active` hands `verify_candidate` in a binary.
    // Built by hand rather than through the registry because the registry
    // is not what is under test here: `verify_candidate` reads `source`,
    // `ddl` and `version` and nothing else.
    let stored = ordo_exec::registry::Artifact {
        version: "r2a-candidate".into(),
        artifact: String::new(),
        source: include_str!("../../ordo-conformance/fixtures/market.ordo").to_string(),
        ddl: seeder.ddl.clone(),
        applied_at: ordo_commandlog::Timestamp(1),
        actor: "operator:r2a".into(),
    };
    let verified = ordo_host::deploy::verify_candidate(
        &stored,
        &pool,
        &cfg(DB),
        operator.clone(),
        space.clone(),
    )
    .await
    .expect("the market fixture gates against its own schema");

    // Half one: identity. `verify_candidate` asserts this internally too;
    // asserting it here is the second independent mechanism, and it is the
    // one that would survive somebody weakening the internal assertion.
    assert!(
        verified.custody_is(&operator),
        "the loaded program does not seal through the supplied provider"
    );
    let constant: Arc<dyn KeyProvider> = Arc::new(ordo_crypto::TestProvider::with_space(
        ordo_host::DEFAULT_SPACE,
    ));
    assert!(
        !verified.custody_is(&constant),
        "the loaded program still seals through the repository's test constant"
    );

    // Half two: the bytes. A write through the door, then the column.
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let deployment = ordo_host::deploy::Deployment::boot(verified);
    let node = boot_on(deployment, &pg, DB, pool, false, metrics, None).await;

    let resp = node
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/op/setMemo")
                .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
                .header("authorization", "Bearer member:mem_1")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "order": "ord_1", "memo": "r2a-plaintext" }).to_string(),
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

    let rows = pg
        .query(r#"SELECT encode("memo", 'hex') FROM "orders" WHERE "id" = 'ord_1'"#)
        .expect("the order is there");
    let sealed = unhex(rows[0][0].as_deref().expect("a sealed memo"));
    assert!(
        !String::from_utf8_lossy(&sealed).contains("r2a-plaintext"),
        "the memo reached the column in the clear"
    );

    // It opens under the operator's custody…
    let lease = operator
        .release(&space, Purpose::Read, "r2a")
        .expect("the operator holds custody");
    assert_eq!(
        operator
            .open_at(
                &lease,
                "orders",
                "memo",
                ordo_crypto::Anchor::Row("ord_1"),
                &sealed
            )
            .expect("the operator's own provider opens what the door sealed"),
        b"r2a-plaintext",
    );

    // …and not under the constant published in this repository. Any error
    // is the pass; `Seal` is the one it should be, and it says nothing
    // about which byte was wrong.
    let tp = ordo_crypto::TestProvider::with_space(ordo_host::DEFAULT_SPACE);
    let tl = tp
        .release(&space, Purpose::Read, "r2a")
        .expect("test lease");
    let refused = tp.open_at(
        &tl,
        "orders",
        "memo",
        ordo_crypto::Anchor::Row("ord_1"),
        &sealed,
    );
    assert!(
        matches!(refused, Err(ordo_crypto::KeyError::Seal)),
        "the repository's test key opened a production write: {refused:?}"
    );
}

// ── 8o: health, catalog, metrics ─────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn the_deployment_surface_is_mounted() {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_host_surface");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let node = boot(&f, &pg, "ordo_host_surface", false, metrics.clone(), None).await;

    // Liveness: no credential, no dependency touched.
    let (st, body) = get(node.router(), "/health/live", None).await;
    assert_eq!((st, body.contains("\"live\"")), (200, true), "{body}");

    // Readiness means what the spec says it means, and it passes only
    // because the shape gate passed above.
    let (st, body) = get(node.router(), "/health/ready", None).await;
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"ready\""), "{body}");

    // The catalog, in both renderings, at the well-known endpoint.
    let (st, json) = get(
        node.router(),
        "/.well-known/ordo-catalog",
        Some("member:mem_1"),
    )
    .await;
    assert_eq!(st, 200, "{json}");
    let catalog: serde_json::Value = serde_json::from_str(&json).expect("json rendering");
    assert!(
        catalog.as_array().is_some_and(|a| a.len() > 20),
        "the catalog looks empty for a member"
    );
    assert!(
        json.contains("guards") && json.contains("cost"),
        "the manifest's disclosures are missing from the catalog"
    );
    let (st, prose) = get(
        node.router(),
        "/.well-known/ordo-catalog?format=prose",
        Some("member:mem_1"),
    )
    .await;
    assert_eq!(st, 200);
    assert!(
        prose.starts_with("# Callable operations"),
        "the prose rendering is not the LLM-ready document: {}",
        &prose[..prose.len().min(120)]
    );
    assert!(prose.contains("guard on tasks"), "{prose}");
    // The short alias is the same document.
    let (_, alias) = get(node.router(), "/catalog", Some("member:mem_1")).await;
    assert_eq!(alias, json, "the alias must serve the same catalog");

    // Unauthenticated: an empty catalog, never an enumeration of what
    // exists. Omitting rather than marking is the whole of F6's scoping.
    let (st, anon) = get(node.router(), "/catalog", None).await;
    assert_eq!((st, anon.as_str()), (200, "[]"), "{anon}");

    // The **interface descriptor** (B1), beside the catalog and scoped by the
    // same check: two documents over one filtered op list. The route is
    // asserted here rather than only in `tests/interface_surface.rs` because
    // a serving surface that nothing mounts is exactly the state 8o found the
    // catalog in.
    let (st, json) = get(
        node.router(),
        "/.well-known/ordo-interface",
        Some("member:mem_1"),
    )
    .await;
    assert_eq!(st, 200, "{json}");
    let iface: serde_json::Value = serde_json::from_str(&json).expect("json rendering");
    assert_eq!(
        iface["schema_version"],
        ordo_plan::interface::SCHEMA_VERSION
    );
    assert!(
        iface["digest"].is_string(),
        "an undigested contract: {json}"
    );
    // Its op list IS the catalog's, at the route as well as in the library.
    let listed: Vec<&str> = catalog
        .as_array()
        .expect("an array")
        .iter()
        .filter_map(|m| m["op"].as_str())
        .collect();
    let described: Vec<&str> = iface["ops"]
        .as_array()
        .expect("ops")
        .iter()
        .filter_map(|o| o["op"].as_str())
        .collect();
    assert_eq!(described, listed, "two surfaces from one door");
    let (_, alias) = get(node.router(), "/interface", Some("member:mem_1")).await;
    assert_eq!(alias, json, "the alias must serve the same descriptor");

    // Anonymous: a well-formed EMPTY document rather than a refusal, so a
    // build step without credentials fails on "nothing to generate".
    let (st, anon) = get(node.router(), "/interface", None).await;
    assert_eq!(st, 200, "{anon}");
    let anon: serde_json::Value = serde_json::from_str(&anon).expect("still json");
    assert_eq!(anon["ops"], serde_json::json!([]));
    assert_eq!(anon["entities"], serde_json::json!([]));
}

/// P3: `/health/deep` counts the tenants people hold of their own, and only
/// the active ones. A tenant made on somebody's say-so, a failed one and one
/// still being built are not among them.
#[tokio::test(flavor = "multi_thread")]
async fn deep_health_counts_own_tenants() {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    use ordo_identity::{TokenKey, TokenSpec};

    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_host_deep_own");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let stream = Arc::new(ordo_secevent::Recorder::new(ordo_secevent::Collapser::new(
        0, 256,
    )));
    let mut node = boot(
        &f,
        &pg,
        "ordo_host_deep_own",
        false,
        metrics.clone(),
        Some(stream.clone()),
    )
    .await;
    let key = TokenKey::from_secret(b"deep-own-test");
    let engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    let access = Arc::new(ordo_host::IdentityAccess {
        engine: std::sync::Arc::new(engine),
        key: TokenKey::from_secret(b"deep-own-test"),
        now: Box::new(|| 1_000),
        stream: stream.clone(),
        metrics: metrics.clone(),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });
    node.platform.access = access.clone();
    node.api.access = access.clone();
    let token = ordo_identity::mint(
        &key,
        &TokenSpec {
            principal: PrincipalId("mem_o".into()),
            expires: Timestamp(9_999),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    );
    let pool = node.platform.pool.clone();
    ordo_host::tenants::ensure_table(&pool)
        .await
        .expect("$spaces");
    pool.rows("DELETE FROM \"$spaces\"").await.expect("clean");

    let (st, body) = get(node.router(), "/health/deep", Some(&token)).await;
    assert_eq!(st, 200, "{body}");
    let seen: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(seen["own_tenants"], 0, "{body}");

    // Two people with an active own tenant, one whose is still being built,
    // one failed, and one tenant a deployment owner made for somebody.
    for (id, owner, own) in [
        ("spc_own1", "ada", Some("ada")),
        ("spc_own2", "grace", Some("grace")),
        ("spc_own3", "edsger", Some("edsger")),
        ("spc_own4", "alan", Some("alan")),
        ("spc_made", "someone", None),
    ] {
        assert!(
            ordo_host::tenants::reserve(&pool, id, id, owner, 1_000, own)
                .await
                .expect("reserve")
        );
    }
    for id in ["spc_own1", "spc_own2", "spc_made"] {
        assert!(ordo_host::tenants::activate(&pool, id, 1_001)
            .await
            .expect("activate"));
    }
    ordo_host::tenants::fail(&pool, "spc_own4", "test", 1_002)
        .await
        .expect("fail");
    let (st, body) = get(node.router(), "/health/deep", Some(&token)).await;
    assert_eq!(st, 200, "{body}");
    let seen: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(seen["own_tenants"], 2, "{body}");
    // Owner, 2026-10-21: the one still being set up is counted, and once
    // the sweep reports it stuck it is named, by id and never by person.
    assert_eq!(seen["tenant_setup"]["in_progress"], 1, "{body}");
    assert_eq!(
        seen["tenant_setup"]["stuck"],
        serde_json::json!([]),
        "{body}"
    );
    assert!(ordo_host::tenants::mark_stuck(&pool, "spc_own3", 1_400)
        .await
        .expect("stuck"));
    let (_, body) = get(node.router(), "/health/deep", Some(&token)).await;
    let seen: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        seen["tenant_setup"]["stuck"][0]["space"], "spc_own3",
        "{body}"
    );
    assert_eq!(
        seen["tenant_setup"]["stuck_after_setting"], "ORDO_PROVISION_STUCK_SECS",
        "{body}"
    );
    // Owner, 2026-10-21: a stuck setup marks the deployment degraded.
    assert_eq!(seen["status"], "degraded", "{body}");
    assert!(!body.contains("edsger"), "no person named: {body}");
}

/// **The self-serve settings read back on `/health/deep`** (owner ruling
/// 2026-10-05): the mode and the two limits. The claim code is removed
/// (owner, 2026-10-30): a row it left is never read back.
#[tokio::test(flavor = "multi_thread")]
async fn deep_health_reads_back_self_serve_and_no_claim_code() {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    use ordo_identity::{TokenKey, TokenSpec};

    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_host_deep_selfserve");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let stream = Arc::new(ordo_secevent::Recorder::new(ordo_secevent::Collapser::new(
        0, 256,
    )));
    let mut node = boot(
        &f,
        &pg,
        "ordo_host_deep_selfserve",
        false,
        metrics.clone(),
        Some(stream.clone()),
    )
    .await;
    let engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    let access = Arc::new(ordo_host::IdentityAccess {
        engine: std::sync::Arc::new(engine),
        key: TokenKey::from_secret(b"deep-selfserve-test"),
        now: Box::new(|| 1_000),
        stream: stream.clone(),
        metrics: metrics.clone(),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });
    node.platform.access = access.clone();
    node.api.access = access.clone();
    let token = ordo_identity::mint(
        &TokenKey::from_secret(b"deep-selfserve-test"),
        &TokenSpec {
            principal: PrincipalId("mem_o".into()),
            expires: Timestamp(9_999),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    );
    let pool = node.platform.pool.clone();
    ordo_host::admission::ensure_table(&pool)
        .await
        .expect("settings");
    ordo_host::self_serve::ensure_table(&pool)
        .await
        .expect("signups");
    pool.rows("DELETE FROM \"$deploymentsetting\"")
        .await
        .expect("clean");

    // Nothing stored: the defaults.
    let (st, body) = get(node.router(), "/health/deep", Some(&token)).await;
    assert_eq!(st, 200, "{body}");
    let seen: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        seen["self_serve"],
        serde_json::json!({"mode": "off", "signupsPerHour": "60"}),
        "{body}"
    );

    // Set, then read back: the mode, the limits, and `required` for a code.
    ordo_host::self_serve::SelfServeStore::new(pool.clone())
        .set(
            Some(ordo_host::self_serve::SelfServe::OnSignup),
            Some(ordo_host::self_serve::Limit::Unlimited),
            "mem_o",
            1_000,
        )
        .await
        .expect("set");
    // A row the removed claim code left behind.
    pool.rows(
        "INSERT INTO \"$deploymentsetting\" (\"key\", \"value\", \"updated_by\", \"updated_at\") \
         VALUES ('first_comer_claim_code', 'required:deadbeefcafe', 'environment', 7)",
    )
    .await
    .expect("old row");
    let (st, body) = get(node.router(), "/health/deep", Some(&token)).await;
    assert_eq!(st, 200, "{body}");
    let seen: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(seen["self_serve"]["mode"], "on-signup", "{body}");
    assert_eq!(seen["self_serve"]["signupsPerHour"], "unlimited", "{body}");
    assert!(seen["self_serve"].get("claimCode").is_none(), "{body}");
    assert!(!body.contains("deadbeefcafe"), "{body}");
}

/// `/health/deep` names the deployment's KeyProvider and its database, so
/// it is access-controlled — and the check is the deployment's own.
#[tokio::test(flavor = "multi_thread")]
async fn health_deep_is_access_controlled() {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    use ordo_identity::{TokenKey, TokenSpec};

    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_host_deep");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    // 7c: the door's recorder is built first and handed to `boot`, so the
    // drain this node runs is draining the recorder the door actually
    // writes into. A drain over a second recorder nothing feeds would pass
    // every assertion below and be worthless.
    let stream = Arc::new(ordo_secevent::Recorder::new(ordo_secevent::Collapser::new(
        0, 256,
    )));
    let mut node = boot(
        &f,
        &pg,
        "ordo_host_deep",
        false,
        metrics.clone(),
        Some(stream.clone()),
    )
    .await;

    // The real door, over verified claims — the conformance `member:<id>`
    // form asserts a principal rather than proving one, and an
    // access-control assertion made against an asserted principal proves
    // nothing.
    let key = TokenKey::from_secret(b"deep-test");
    let engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    let access = Arc::new(ordo_host::IdentityAccess {
        engine: std::sync::Arc::new(engine),
        key: TokenKey::from_secret(b"deep-test"),
        now: Box::new(|| 1_000),
        stream: stream.clone(),
        metrics: metrics.clone(),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });
    node.platform.access = access.clone();
    node.api.access = access.clone();
    let token = |id: &str| {
        ordo_identity::mint(
            &key,
            &TokenSpec {
                principal: PrincipalId(id.into()),
                expires: Timestamp(9_999),
                client: None,
                user_presence_at: None,
                capability: None,
                delegation: None,
                session: None,
            },
        )
    };

    // No credential at all.
    let (st, body) = get(node.router(), "/health/deep", None).await;
    assert_eq!(st, 403, "an unauthenticated caller got deep health: {body}");
    assert!(
        !body.contains("key_provider") && !body.contains("current_epoch"),
        "the refusal leaked the detail it exists to protect: {body}"
    );

    // A garbage credential answers exactly the same way.
    let (st2, body2) = get(node.router(), "/health/deep", Some("not-a-token")).await;
    assert_eq!((st, body), (st2, body2), "the refusal is one answer");

    // A real member who is not an owner: still refused. `mem_1` is a
    // manager in the fixture, so this separates "authenticated" from
    // "authorized" — the token verifies and the door still says no.
    let (st, body) = get(node.router(), "/health/deep", Some(&token("mem_1"))).await;
    assert_eq!(st, 403, "a non-owner got deep health: {body}");

    // The owner gets it, and it is the detail the endpoint exists for.
    let (st, body) = get(node.router(), "/health/deep", Some(&token("mem_o"))).await;
    assert_eq!(
        st, 200,
        "the owner must be able to read deep health: {body}"
    );
    let deep: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(deep["status"], "ok", "{body}");
    assert_eq!(deep["asked_by"], "mem_o");
    assert!(
        deep["key_provider"]["reachable"].as_bool().unwrap(),
        "{body}"
    );
    assert!(
        deep["key_provider"]["provider"].as_str().is_some(),
        "deep health must name the KeyProvider — that is why it is gated"
    );
    // It publishes the load gate's coverage, so an operator reads it rather
    // than trusting it.
    assert!(deep["shape_verification"]["summary"]
        .as_str()
        .unwrap()
        .contains("shape-verified"));

    // `/metrics` is gated the same way and for its own reason: the series
    // carry principal ids.
    let (st, _) = get(node.router(), "/metrics", None).await;
    assert_eq!(st, 403, "metrics must not be public");
    let (st, text) = get(node.router(), "/metrics", Some(&token("mem_o"))).await;
    assert_eq!(st, 200);

    // 7b, the mandatory alert's substrate: the two denials above are
    // countable, per principal and per space.
    assert!(
        text.contains("ordo_denials_total"),
        "no denial series at all:\n{text}"
    );
    assert!(
        text.contains("principal=\"mem_1\"") && text.contains("space=\"conformance\""),
        "the denial is not attributed per principal per space:\n{text}"
    );
    assert!(
        text.contains("principal=\"unknown\",principal_kind=\"unverified\""),
        "an unverifiable token must still be counted, in one bucket:\n{text}"
    );
    // And it never carries what telemetry must not carry.
    assert!(!text.contains("Bearer"), "a bearer token reached a label");
    assert!(!text.contains(&token("mem_o")), "a token reached a label");

    // ── 7c: the denials above reach durable storage ──────────────────────
    //
    // Two artifacts, one act. The counters asserted above are the
    // *alerting* substrate; this is the *tamper-evident record*, and until
    // now nothing on any host path moved an entry from one to the other.
    let pool = ordo_exec::Pool::connect(cfg("ordo_host_deep"))
        .await
        .expect("a pool of this test's own");
    let stored = || {
        let pool = pool.clone();
        async move {
            pool.rows("SELECT count(*) FROM \"$secevents\"")
                .await
                .unwrap()[0][0]
                .as_deref()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0)
        }
    };
    let mut written = 0;
    for _ in 0..200 {
        written = stored().await;
        if written > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        written > 0,
        "the door recorded denials and the drain never wrote one — the \
         security stream is not reaching storage on this host path"
    );

    // The unverifiable presentation went to the DEPLOYMENT stream and the
    // member's denial to the space's, which is §6.1's members-only rule
    // surviving the whole assembly rather than only the unit test.
    let scopes = pool
        .rows("SELECT DISTINCT \"scope\" FROM \"$secevents\" ORDER BY \"scope\"")
        .await
        .unwrap();
    let scopes: Vec<String> = scopes.into_iter().filter_map(|r| r[0].clone()).collect();
    assert!(
        scopes.iter().any(|s| s == ordo_secevent::DEPLOYMENT_KEY),
        "an unverifiable presentation must land in the deployment stream: \
         {scopes:?}"
    );

    // And `/health/deep` answers for the record as well as for the
    // database: an operator reading it learns whether the audit stream is
    // being written, which is the question the old surface could not
    // answer at all.
    let (st, body) = get(node.router(), "/health/deep", Some(&token("mem_o"))).await;
    assert_eq!(st, 200, "{body}");
    let deep: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        deep["security_stream"]["wired"], true,
        "a deployment with a drain must say so: {body}"
    );
    assert_eq!(deep["security_stream"]["append_failures"], 0, "{body}");
    assert!(
        deep["security_stream"]["appended"].as_u64().unwrap_or(0) > 0,
        "{body}"
    );
    assert!(
        deep["security_stream"]["retention"]
            .as_str()
            .unwrap_or_default()
            .contains("INACTIVE"),
        "a deployment with no security signing key prunes nothing, and the \
         surface must say so rather than looking healthy: {body}"
    );
}

// ── D0: the platform surfaces stop authorizing against a caller-named
// space ─────────────────────────────────────────────────────────────────

/// `deployment_engine` hardcodes its space to `ordo_host::DEFAULT_SPACE`
/// (`lib.rs:1317`, `SpaceId(SPACE.to_string())`), so it cannot itself
/// provision a second space: there is no in-tree deployment path that
/// builds one (§0.7 of `plan-opslog.md` says the same). What a multi-space
/// deployment's `AccessEngine` looks like is not in question, though: it is
/// exactly a second `(space, principal) -> Membership` entry and a second
/// `AccessProfile`, the same two primitives `deployment_engine`'s own loop
/// calls for the first space. So this adds a second space to the real
/// engine the door actually checks against, by calling the same
/// `set_membership` / `set_profile` the builder calls, rather than reading
/// a second `members` table nothing provisions. From `IdentityAccess::check`
/// downward, which is the code the fix and the defect both live in, this is
/// indistinguishable from a deployment that provisioned the second space
/// some other way.
///
/// Returns the intruder's own token: an owner of `spc_intruder`, a member
/// of nothing in `ordo_host::DEFAULT_SPACE`.
fn add_intruder_space(engine: &mut ordo_identity::AccessEngine) -> (String, String) {
    use ordo_identity::principal::{AccessProfile, PrincipalId, Role, SpaceId};

    let other = SpaceId("spc_intruder".into());
    let intruder = PrincipalId("intruder".into());
    engine.set_membership(&other, &intruder, Role::Owner);
    let mut profile = AccessProfile::new();
    for op in [
        ordo_host::serve::HEALTH_DEEP_OP,
        ordo_host::serve::METRICS_OP,
        ordo_host::serve::ALERTS_OP,
        // 7d(b). Mirrors `engine_from_members`, which installs one profile
        // in **every** space, so a tenant owner really does hold this op in
        // their own space. A helper that left it out would make the
        // owner-reads-their-own-space test fail for a reason production does
        // not have.
        ordo_host::serve::SECEVENTS_OP,
    ] {
        profile = profile.allow(Role::Owner, op);
    }
    engine.set_profile(&other, profile);
    (other.0, intruder.0)
}

/// `GET` with an explicit (or absent) `x-ordo-space` header, rather than
/// `ordo_host::DEFAULT_SPACE` as `get()` hardcodes: so a test can name a space that
/// is not the deployment's own, or name none at all and let `$space` in the
/// query string carry it instead. `None` omits the header entirely rather
/// than sending it empty, which is a third, meaningless case `space_of`
/// would otherwise have to be asked about.
async fn get_in_space(
    router: axum::Router,
    path: &str,
    space: Option<&str>,
    token: Option<&str>,
) -> (u16, String) {
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    let mut req = Request::builder().method("GET").uri(path);
    if let Some(s) = space {
        req = req.header("x-ordo-space", s);
    }
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = router
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// The general form of 0.7's defect: a reserved platform op checked against
/// a space the caller names, rather than against the deployment's own.
/// Exercised on `/health/deep` through the `x-ordo-space` header and again
/// through the `$space` query parameter, `space_of`'s two caller-supplied
/// sources, so a fix that only closed one would still leave this failing.
///
/// The intruder owns `spc_intruder` and nothing else. Under the defect,
/// naming that space is enough to pass a check that is supposed to be about
/// `ordo_host::DEFAULT_SPACE`, the space `/health/deep` actually describes.
#[tokio::test(flavor = "multi_thread")]
async fn a_platform_op_is_not_authorized_by_a_caller_named_space() {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    use ordo_identity::{TokenKey, TokenSpec};

    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_host_caller_space");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let mut node = boot(
        &f,
        &pg,
        "ordo_host_caller_space",
        false,
        metrics.clone(),
        None,
    )
    .await;

    let key = TokenKey::from_secret(b"caller-space-test");
    let mut engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    let (other_space, intruder) = add_intruder_space(&mut engine);
    let access = Arc::new(ordo_host::IdentityAccess {
        engine: std::sync::Arc::new(engine),
        key: TokenKey::from_secret(b"caller-space-test"),
        now: Box::new(|| 1_000),
        stream: Arc::new(ordo_secevent::Recorder::default()),
        metrics: metrics.clone(),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });
    node.platform.access = access.clone();
    node.api.access = access;
    let token = ordo_identity::mint(
        &key,
        &TokenSpec {
            principal: PrincipalId(intruder),
            expires: Timestamp(9_999),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    );

    // Naming the space they actually own, in the header `space_of` reads
    // first.
    let (st, body) = get_in_space(
        node.router(),
        "/health/deep",
        Some(&other_space),
        Some(&token),
    )
    .await;
    assert_eq!(
        st, 403,
        "an owner of a DIFFERENT space read deep health for this one: {body}"
    );
    assert!(
        !body.contains("key_provider") && !body.contains("current_epoch"),
        "the refusal leaked the detail it exists to protect: {body}"
    );

    // Naming it in `$space` instead, with no header at all: `space_of`'s
    // second source, checked only when the header is absent.
    let (st, body) = get_in_space(
        node.router(),
        &format!("/health/deep?$space={other_space}"),
        None,
        Some(&token),
    )
    .await;
    assert_eq!(
        st, 403,
        "the query parameter reached the same caller-named-space bug: {body}"
    );
}

/// §0.7: `platform.metrics` is granted to `Role::Owner`, and its series
/// carry principal ids and op names across every space this process served
/// (`metrics.rs:107-112`). An owner of one space must not be able to read
/// another's by naming it.
///
/// Needs a second space in the same deployment's `AccessEngine`, the
/// situation the defect requires. `deployment_engine` cannot build one
/// itself (see `add_intruder_space`'s doc), so this test adds the second
/// space to the real engine with the engine's own primitives and drives a
/// real HTTP request through the real router and the real
/// `IdentityAccess::check`. What is NOT exercised is a `members` table that
/// independently provisioned the second space: no such deployment exists
/// in-tree for this builder, which is §0.7's own finding, restated here as a
/// property of the test rather than of the code.
#[tokio::test(flavor = "multi_thread")]
async fn metrics_denies_an_owner_of_another_space() {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    use ordo_identity::{TokenKey, TokenSpec};

    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_host_metrics_other_space");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let mut node = boot(
        &f,
        &pg,
        "ordo_host_metrics_other_space",
        false,
        metrics.clone(),
        None,
    )
    .await;

    let key = TokenKey::from_secret(b"metrics-other-space-test");
    let mut engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    let (other_space, intruder) = add_intruder_space(&mut engine);
    let access = Arc::new(ordo_host::IdentityAccess {
        engine: std::sync::Arc::new(engine),
        key: TokenKey::from_secret(b"metrics-other-space-test"),
        now: Box::new(|| 1_000),
        stream: Arc::new(ordo_secevent::Recorder::default()),
        metrics: metrics.clone(),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });
    node.platform.access = access.clone();
    node.api.access = access;
    let token = ordo_identity::mint(
        &key,
        &TokenSpec {
            principal: PrincipalId(intruder),
            expires: Timestamp(9_999),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    );

    // The intruder is a real owner, of `spc_intruder`, not of
    // `ordo_host::DEFAULT_SPACE`. Naming their own space must not open the
    // deployment's cross-tenant metrics.
    let (st, body) =
        get_in_space(node.router(), "/metrics", Some(&other_space), Some(&token)).await;
    assert_eq!(
        st, 403,
        "the owner of another space read this deployment's metrics: {body}"
    );
    assert!(
        !body.contains("ordo_denials_total") && !body.contains("principal="),
        "a denial leaked cross-tenant metrics content rather than refusing outright: {body}"
    );
}

/// D0's second defect: `/security/alerts` authorizes against
/// `platform.security.alerts`, and nothing deployed that op, so it denied
/// everyone in every deployment built through `deployment_engine`, with no
/// test over the route anywhere in this crate before this one.
#[tokio::test(flavor = "multi_thread")]
async fn the_alerts_route_answers_the_deployment_owner() {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    use ordo_identity::{TokenKey, TokenSpec};

    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_host_alerts_route");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let mut node = boot(
        &f,
        &pg,
        "ordo_host_alerts_route",
        false,
        metrics.clone(),
        None,
    )
    .await;

    let key = TokenKey::from_secret(b"alerts-route-test");
    let engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    let access = Arc::new(ordo_host::IdentityAccess {
        engine: std::sync::Arc::new(engine),
        key: TokenKey::from_secret(b"alerts-route-test"),
        now: Box::new(|| 1_000),
        stream: Arc::new(ordo_secevent::Recorder::default()),
        metrics: metrics.clone(),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });
    node.platform.access = access.clone();
    node.api.access = access;
    let token = |id: &str| {
        ordo_identity::mint(
            &key,
            &TokenSpec {
                principal: PrincipalId(id.into()),
                expires: Timestamp(9_999),
                client: None,
                user_presence_at: None,
                capability: None,
                delegation: None,
                session: None,
            },
        )
    };

    // No credential: refused, same shape as the other two platform surfaces.
    let (st, body) = get(node.router(), "/security/alerts", None).await;
    assert_eq!(st, 403, "the alerts route must not be public: {body}");

    // A non-owner member: still refused. `mem_1` is a manager in the
    // fixture.
    let (st, body) = get(node.router(), "/security/alerts", Some(&token("mem_1"))).await;
    assert_eq!(st, 403, "a non-owner read the alert surface: {body}");

    // The deployment owner reaches it, and gets the rendering
    // `ordo_secevent::alert::DenyVolumeMonitor::render` produces: the
    // mandatory rule, whether the deployment configured it, and the scope
    // caveat, over `text/plain` per the route's own doc.
    let (st, body) = get(node.router(), "/security/alerts", Some(&token("mem_o"))).await;
    assert_eq!(
        st, 200,
        "the deployment owner must be able to read the mandatory alert: {body}"
    );
    assert!(
        body.contains("deny-volume alert"),
        "not the alert rendering: {body}"
    );
    assert!(
        body.contains("rule:"),
        "the rule itself must be named: {body}"
    );
    // This harness's `Metrics::new()` reads `DenyVolumeRule::from_env`
    // (`metrics.rs:285`) with neither `ORDO_DENY_ALERT_THRESHOLD` nor
    // `ORDO_DENY_ALERT_WINDOW_SECS` set, so the rule runs on the built-in
    // default and says so rather than looking configured.
    assert!(
        body.contains("MISCONFIGURED"),
        "an unconfigured rule must say so rather than looking deliberate: {body}"
    );
}

/// The key-release counter has something real to count: sealing a value
/// takes a write-purpose release, and it lands in the substrate.
#[tokio::test(flavor = "multi_thread")]
async fn key_releases_are_counted() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let f = metered_fixture(&metrics);
    let pg = make_db("ordo_host_keys");
    f.reset(&pg).expect("seeded");
    let node = boot(&f, &pg, "ordo_host_keys", false, metrics.clone(), None).await;

    let releases = || -> u64 {
        metrics
            .snapshot()
            .into_iter()
            .filter(|(n, _, _)| n == ordo_host::metrics::KEY_RELEASES)
            .map(|(_, _, v)| v)
            .sum()
    };
    let before = releases();

    // `setMemo` writes a sealed field, so the write protocol seals under a
    // write-purpose release.
    let resp = node
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/op/setMemo")
                .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
                .header("authorization", "Bearer member:mem_1")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "order": "ord_1", "memo": "counted" }).to_string(),
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

    assert!(
        releases() > before,
        "a sealing write released no countable key: {:?}",
        metrics.snapshot()
    );
    let text = metrics.render_prometheus();
    assert!(
        text.contains("ordo_key_release_total{purpose=\"write\",space=\"conformance\"}"),
        "the release is not attributed by space and purpose:\n{text}"
    );
    // Envelope-tier: a release names the space and why, never the value it
    // sealed.
    assert!(
        !text.contains("counted"),
        "a sealed value reached a label:\n{text}"
    );
}

/// 5f: `ordo_op_duration_seconds` reaches `/metrics` through the real door,
/// on a real dispatch, in the exposition format a scraper reads.
///
/// The unit tests in `metrics.rs` prove the arithmetic and
/// `tests/lock_wait.rs` proves the lock-wait family sees a contended lock.
/// What is left, and what this covers, is the wiring: the door's executor
/// is wrapped and the series comes out of the endpoint an operator scrapes
/// — a line somebody can delete without any other test noticing.
#[tokio::test(flavor = "multi_thread")]
async fn a_dispatched_write_publishes_both_histogram_families() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let f = metered_fixture(&metrics);
    let pg = make_db("ordo_host_hist");
    f.reset(&pg).expect("seeded");
    let node = boot(&f, &pg, "ordo_host_hist", false, metrics.clone(), None).await;

    let resp = node
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/op/setMemo")
                .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
                .header("authorization", "Bearer member:mem_1")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "order": "ord_1", "memo": "timed" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let text = metrics.render_prometheus();
    // The p99 series, labelled by op and by outcome.
    assert!(
        text.contains("# TYPE ordo_op_duration_seconds histogram"),
        "no op-duration family:\n{text}"
    );
    assert!(
        text.contains("ordo_op_duration_seconds_count{op=\"setMemo\",outcome=\"ok\"} 1"),
        "the dispatch was not timed by op and outcome:\n{text}"
    );
    // And the lock-wait family is **present**: this harness's pool reports
    // to the metrics, and the write above took the append lock, because the
    // host's writes now join the durable chain in their own transaction
    // (checklist 11-28). This used to assert the family was silent, which
    // was the honest answer while the host built groups in memory only.
    assert!(
        text.contains("ordo_chain_append_lock_wait_seconds"),
        "the write appended to the durable chain, so the append lock was taken \
         and the instrument should have seen it:\n{text}"
    );
    // Envelope-tier, like every other series: the memo it sealed is not a
    // label and neither is the credential that wrote it.
    assert!(!text.contains("timed"), "a written value reached a label");
    assert!(!text.contains("Bearer"), "a credential reached a label");
}

// ── 9j + 8p: liveness, and whether it crosses nodes ──────────────────────

/// DL §5: named ⇒ callable ⇒ sourceable ⇒ subscribable, and there is no
/// definition flag. So every read is live-capable and no write is.
#[test]
fn every_named_query_is_live_capable_and_no_write_is() {
    use ordo_api::OpKind;

    let f = Fixture::load().expect("fixture");
    let specs = ordo_host::build_specs(&f);
    assert!(specs.len() > 40, "the fixture shrank");
    for s in &specs {
        assert_eq!(
            s.live_capable,
            s.kind == OpKind::Read,
            "`{}` ({:?}) disagrees with DL §5",
            s.name,
            s.kind
        );
    }
    // Composition included: a named query sourcing another is still named.
    assert!(specs
        .iter()
        .any(|s| s.name == "specTasks" && s.live_capable));
}

/// The index that turns "entity `x` changed" into "op `y` is stale".
/// G16: guard entities are inlined base entities and are subscribed by
/// `?live`, so a membership write invalidates a guarded live query.
#[test]
fn the_invalidation_index_includes_guard_entities() {
    let f = Fixture::load().expect("fixture");
    let subscribed = ordo_host::invalidation::subscribed_entities(&f, "openTasks");
    assert!(subscribed.contains("tasks"), "{subscribed:?}");
    // Reached only through `tasks`' guard walk: project.projectMembers(...).
    assert!(
        subscribed.contains("projects") && subscribed.contains("projectMembers"),
        "guard entities are missing, so a permission change would not \
         invalidate a guarded live query (G16): {subscribed:?}"
    );
    // Composition flattens: a composed op subscribes to what it inlined.
    let composed = ordo_host::invalidation::subscribed_entities(&f, "specTasks");
    assert!(composed.contains("tasks"), "{composed:?}");

    let index = ordo_host::invalidation::entity_index(&f);
    assert!(index["tasks"].iter().any(|o| o == "openTasks"));
    assert!(
        index["projectMembers"].iter().any(|o| o == "openTasks"),
        "a membership write must reach openTasks' subscribers"
    );
    // Write ops are not subscribable, so nothing ever publishes to one.
    assert!(index
        .values()
        .all(|ops| !ops.iter().any(|o| o == "addTask")));
}

/// 8p, the point of the whole item: a write committed through **one** pool
/// and executor must reach a subscriber attached to a **different** one.
///
/// Node A and node B share a database and share nothing else — separate
/// pools, separate executors, separate buses, separate LISTEN sessions.
/// This is as close to two processes as one test binary gets, and a test
/// that only exercised one process's broadcast channel would prove nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_on_one_node_reaches_a_live_subscriber_on_another() {
    use axum::body::Body;
    use axum::http::Request;
    use futures::StreamExt;
    use tower::util::ServiceExt;

    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_host_cross");
    f.reset(&pg).expect("seeded");
    let metrics_a = Arc::new(ordo_host::metrics::Metrics::new());
    let metrics_b = Arc::new(ordo_host::metrics::Metrics::new());
    let node_a = boot(&f, &pg, "ordo_host_cross", true, metrics_a, None).await;
    let node_b = boot(&f, &pg, "ordo_host_cross", true, metrics_b.clone(), None).await;

    // Node B holds the live subscription.
    let resp = node_b
        .router()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/op/openTasks?live=1")
                .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
                .header("authorization", "Bearer member:mem_1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "the subscription was refused");
    assert_eq!(
        resp.headers()["content-type"],
        "text/event-stream",
        "not an SSE stream"
    );
    let mut stream = resp.into_body().into_data_stream();

    // Frame one is the ordinary envelope at the op's full scope.
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next())
        .await
        .expect("the first frame never arrived")
        .expect("stream ended")
        .expect("body error");
    let first = String::from_utf8_lossy(&first).into_owned();
    assert!(first.contains("\"op\":\"openTasks\""), "{first}");
    assert!(!first.contains("invalidate"), "{first}");

    // Node A commits a write. Different pool, different executor, and its
    // `pg_notify` rides that write's own transaction.
    let (st, body) = {
        let req = Request::builder()
            .method("POST")
            .uri("/op/addTask")
            .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
            .header("authorization", "Bearer member:mem_1")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "project": "proj_1", "title": "cross-node" }).to_string(),
            ))
            .unwrap();
        let resp = node_a.router().oneshot(req).await.unwrap();
        let st = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (st, String::from_utf8_lossy(&bytes).into_owned())
    };
    assert_eq!(st, 200, "the write on node A failed: {body}");

    // …and node B's subscriber hears about it. This is the assertion 8p is
    // about: nothing in this process's memory carried the signal — it went
    // out through node A's transaction and came back through node B's own
    // LISTEN session.
    let mut saw_invalidate = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        let Ok(Some(Ok(chunk))) =
            tokio::time::timeout(std::time::Duration::from_secs(20), stream.next()).await
        else {
            break;
        };
        let text = String::from_utf8_lossy(&chunk).into_owned();
        if text.contains("event: invalidate") {
            saw_invalidate = true;
            break;
        }
        // Heartbeats are fine; anything else is not.
        assert!(text.contains(":hb"), "unexpected frame: {text}");
    }
    assert!(
        saw_invalidate,
        "node B's subscriber never heard node A's commit — `?live=1` is \
         silently stale across nodes"
    );

    // The signal is a staleness signal and nothing else: no rows travelled.
    // And node B counted it as having arrived over the listener, which is
    // what makes "is the live path actually crossing" answerable in
    // production rather than only here.
    let crossed: u64 = metrics_b
        .snapshot()
        .into_iter()
        .filter(|(n, l, _)| {
            n == ordo_host::metrics::INVALIDATIONS
                && l.iter().any(|(k, v)| k == "source" && v == "listener")
        })
        .map(|(_, _, v)| v)
        .sum();
    assert!(crossed > 0, "node B recorded no listener invalidation");
}

/// 27s: one stream carries several reads, over the real Postgres bus.
///
/// Two reads on one shared stream each hear their own writes and only
/// those; a read the caller may not call is refused and the stream stays
/// open; a dropped read hears nothing more.
#[tokio::test(flavor = "multi_thread")]
async fn one_shared_stream_carries_two_reads_over_the_postgres_bus() {
    use axum::body::Body;
    use axum::http::Request;
    use futures::StreamExt;
    use tower::util::ServiceExt;

    const TOKEN: &str = "Bearer member:mem_1";

    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_host_shared_live");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let node = boot(&f, &pg, "ordo_host_shared_live", true, metrics, None).await;

    async fn call(
        node: &Node,
        method: &str,
        uri: &str,
        space: &str,
        body: serde_json::Value,
    ) -> (u16, String) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-ordo-space", space)
            .header("authorization", TOKEN)
            .header("content-type", "application/json")
            .body(if body.is_null() {
                Body::empty()
            } else {
                Body::from(body.to_string())
            })
            .unwrap();
        let resp = node.router().oneshot(req).await.unwrap();
        let st = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (st, String::from_utf8_lossy(&bytes).into_owned())
    }

    // Open the stream; its first frame names it.
    let resp = node
        .router()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/live")
                .header("authorization", TOKEN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "the shared stream was refused");
    let mut stream = resp.into_body().into_data_stream();
    let mut heard = String::new();
    let next = |heard: &mut String, chunk: axum::body::Bytes| {
        heard.push_str(&String::from_utf8_lossy(&chunk));
    };
    while !heard.contains("\n\n") {
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next())
            .await
            .expect("the ready frame never arrived")
            .expect("stream ended")
            .expect("body error");
        next(&mut heard, chunk);
    }
    assert!(heard.starts_with("event: ready"), "{heard}");
    let data = heard
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .unwrap();
    let id = serde_json::from_str::<serde_json::Value>(data).unwrap()["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let space = ordo_host::DEFAULT_SPACE;
    let add = format!("/live/{id}");

    // Two reads over different entities.
    for (key, op) in [("tasks", "openTasks"), ("boards", "boardList")] {
        let (st, body) = call(
            &node,
            "POST",
            &add,
            space,
            serde_json::json!({ "key": key, "op": op }),
        )
        .await;
        assert_eq!(st, 200, "{op}: {body}");
        assert!(body.contains(&format!("\"op\":\"{op}\"")), "{body}");
    }
    // A space this member is not in: refused, as the op alone would be.
    let (st, body) = call(
        &node,
        "POST",
        &add,
        "spc_nobody",
        serde_json::json!({ "key": "x", "op": "boardList" }),
    )
    .await;
    assert_eq!(st, 403, "{body}");
    assert!(body.contains("access_denied"), "{body}");

    /// Read frames until one names `want`; the keys of every invalidate seen.
    async fn until_key(
        stream: &mut (impl futures::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin),
        want: &str,
    ) -> Vec<String> {
        let mut text = String::new();
        let needle = format!("\"key\":\"{want}\"");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !text.contains(&needle) {
            assert!(
                std::time::Instant::now() < deadline,
                "no signal for {want}; heard {text}"
            );
            let Ok(Some(Ok(chunk))) =
                tokio::time::timeout(std::time::Duration::from_secs(20), stream.next()).await
            else {
                panic!("the stream ended before {want}'s signal; heard {text}");
            };
            text.push_str(&String::from_utf8_lossy(&chunk));
        }
        text.split("\n\n")
            .filter(|f| f.contains("event: invalidate"))
            .filter_map(|f| f.lines().find_map(|l| l.strip_prefix("data: ")))
            .map(|d| {
                serde_json::from_str::<serde_json::Value>(d).unwrap()["key"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    // A task write reaches the tasks read and not the boards read.
    let (st, body) = call(
        &node,
        "POST",
        "/op/addTask",
        space,
        serde_json::json!({ "project": "proj_1", "title": "shared-1" }),
    )
    .await;
    assert_eq!(st, 200, "{body}");
    assert_eq!(until_key(&mut stream, "tasks").await, vec!["tasks"]);

    // A board write reaches the boards read.
    let (st, body) = call(
        &node,
        "POST",
        "/op/addBoard",
        space,
        serde_json::json!({ "name": "shared" }),
    )
    .await;
    assert_eq!(st, 200, "{body}");
    assert_eq!(until_key(&mut stream, "boards").await, vec!["boards"]);

    // Drop the tasks read. A task write then a board write: only the board
    // signal arrives.
    let (st, body) = call(
        &node,
        "DELETE",
        &format!("/live/{id}/tasks"),
        space,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(st, 204, "{body}");
    let (st, _) = call(
        &node,
        "POST",
        "/op/addTask",
        space,
        serde_json::json!({ "project": "proj_1", "title": "shared-2" }),
    )
    .await;
    assert_eq!(st, 200);
    let (st, _) = call(
        &node,
        "POST",
        "/op/addBoard",
        space,
        serde_json::json!({ "name": "shared-2" }),
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(until_key(&mut stream, "boards").await, vec!["boards"]);
}

/// The other half of the same claim: with the in-process bus, node B hears
/// nothing. Stated as a test so the cross-node result above cannot be an
/// accident of some shared channel.
#[tokio::test(flavor = "multi_thread")]
async fn the_in_process_bus_does_not_cross_nodes() {
    use axum::body::Body;
    use axum::http::Request;
    use futures::StreamExt;
    use tower::util::ServiceExt;

    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_host_nocross");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let node_a = boot(&f, &pg, "ordo_host_nocross", false, metrics.clone(), None).await;
    let node_b = boot(&f, &pg, "ordo_host_nocross", false, metrics, None).await;

    let resp = node_b
        .router()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/op/openTasks?live=1")
                .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
                .header("authorization", "Bearer member:mem_1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = resp.into_body().into_data_stream();
    let _first = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next())
        .await
        .expect("first frame")
        .expect("stream ended")
        .expect("body");

    let req = Request::builder()
        .method("POST")
        .uri("/op/addTask")
        .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
        .header("authorization", "Bearer member:mem_1")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "project": "proj_1", "title": "same-node only" }).to_string(),
        ))
        .unwrap();
    let resp = node_a.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    // Nothing but heartbeats reaches B in a window the cross-node test
    // clears comfortably.
    let quiet = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    let text = String::from_utf8_lossy(&chunk).into_owned();
                    if text.contains("event: invalidate") {
                        return false;
                    }
                }
                _ => return true,
            }
        }
    })
    .await;
    assert!(
        quiet.unwrap_or(true),
        "MemoryBus crossed processes, which it cannot — the cross-node test \
         above would then be proving nothing"
    );
}

// ── layer-1 client pinning (8q) ──────────────────────────────────────────

/// Like [`get`], but keeps the headers — which is the whole subject here.
async fn get_headers(
    router: axum::Router,
    path: &str,
    token: Option<&str>,
) -> (u16, axum::http::HeaderMap) {
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    let mut req = Request::builder()
        .method("GET")
        .uri(path)
        .header("x-ordo-space", ordo_host::DEFAULT_SPACE);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = router
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    (resp.status().as_u16(), resp.headers().clone())
}

fn hdr(h: &axum::http::HeaderMap, name: &str) -> Option<String> {
    h.get(name).and_then(|v| v.to_str().ok()).map(String::from)
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn parse_head(s: &str) -> ordo_commandlog::ChainHash {
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
    }
    ordo_commandlog::ChainHash(out)
}

/// `bootstrap.md` calls layer 1 *"always on, ~free, and requires no
/// configuration at all"*, on the strength of responses carrying a
/// command-log position and the signed head at it. They carried neither, so
/// none of it was true.
///
/// What matters is the pin's **verifiability** — a header a client cannot
/// check is decoration. The signature must verify under the space's
/// published key, over the log's own canonical bytes, with the same routine
/// that checks a stored checkpoint.
#[tokio::test(flavor = "multi_thread")]
async fn every_answered_request_carries_a_verifiable_pin() {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_dep_pin");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let node = boot(&f, &pg, "ordo_dep_pin", false, metrics, None).await;

    let (st, h) = get_headers(node.router(), "/op/openTasks", Some("member:mem_1")).await;
    assert_eq!(st, 200);

    let upto: u64 = hdr(&h, "x-ordo-position")
        .expect("a position, or layer 1 does not exist")
        .parse()
        .unwrap();
    let head = hdr(&h, "x-ordo-head").expect("a head");
    let sig = hdr(&h, "x-ordo-head-sig").expect("a signature");
    assert_eq!(
        hdr(&h, "x-ordo-head-key").as_deref(),
        Some("deployment-test")
    );

    let signer = ordo_commandlog::sign::Ed25519Signer::from_seed("deployment-test", &[7u8; 32]);
    let bytes = ordo_commandlog::Checkpoint::signed_bytes(
        &ordo_commandlog::SpaceId(ordo_host::DEFAULT_SPACE.to_string()),
        upto,
        parse_head(&head),
        ordo_commandlog::Timestamp(0),
    );
    assert!(
        ordo_commandlog::store::Signer::verify(&signer, &bytes, &unhex(&sig)),
        "the pin must verify, or a client cannot use it as evidence"
    );

    // A second request pins the same head. The chain has not moved, and a pin
    // that changed without the history changing would be noise a client would
    // learn to ignore.
    let (_, h2) = get_headers(node.router(), "/op/openTasks", Some("member:mem_1")).await;
    assert_eq!(hdr(&h2, "x-ordo-head"), Some(head));
    assert_eq!(
        hdr(&h2, "x-ordo-head-sig"),
        Some(sig),
        "one head signs to one signature, so two replicas agree without \
         coordinating"
    );
}

/// A refusal at the door carries **no** pin, for the reason the version
/// headers already carry none: a position tells a caller the space exists and
/// how much history it has, which is more than somebody who never got in
/// should learn.
#[tokio::test(flavor = "multi_thread")]
async fn a_door_refusal_pins_nothing() {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_dep_pin_door");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let node = boot(&f, &pg, "ordo_dep_pin_door", false, metrics, None).await;
    let (st, h) = get_headers(node.router(), "/op/openTasks", None).await;
    assert!(
        (400..500).contains(&st),
        "expected a door refusal, got {st}"
    );
    assert!(
        hdr(&h, "x-ordo-position").is_none(),
        "an unauthenticated caller learns nothing about the chain"
    );
}

// ── 9k: the std library is registry-present, exposure opt-in ─────────────

/// `std-library.md` §5 rules 2 and 3: deriving an op and exposing it are
/// different acts, and a profile template SHOULD reach owner and manager
/// tiers only.
///
/// This is the rule that decides whether the std library is a convenience or
/// the largest privilege escalation in the platform. `std.E.get` and
/// `std.E.update` are, between them, read-and-write access to every open
/// field of every entity; the guards still filter rows, but a viewer holding
/// the whole derived surface reads every row the guard lets them reach and
/// writes every field of it. The derivation cannot decide that — who may
/// call what is runtime state, never a compile side effect — so the template
/// has to, and the template is exactly the code this asserts against.
///
/// It is a real regression test, not a restatement: this deployment's
/// template grants every op to every role it makes sense for, so the moment
/// std derived, every viewer in the fixture held all 85 derived ops. The
/// carve-out is one condition, and this is the assertion that keeps it.
#[test]
fn the_derived_std_surface_is_not_granted_to_viewers_by_the_template() {
    use ordo_identity::principal::{PrincipalId, SpaceId, Timestamp};
    use ordo_identity::{Decision, TokenClaims};

    let f = Fixture::load().expect("fixture");
    let pg = make_db("ordo_std_exposure");
    f.reset(&pg).expect("seeded");
    let engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    let space = SpaceId(ordo_host::DEFAULT_SPACE.to_string());

    let claims = |id: &str| TokenClaims {
        principal: PrincipalId(id.into()),
        capability: None,
        client: None,
        user_presence_at: None,
        delegation: None,
        session: None,
    };
    let may = |id: &str, op: &str| {
        matches!(
            engine.check(&claims(id), &space, op, Timestamp(1_000)),
            Decision::Allow
        )
    };

    // mem_4 is a viewer, mem_2 an editor, mem_o the owner.
    for op in [
        "std.tasks.get",
        "std.tasks.count",
        "std.tasks.update",
        "std.tasks.delete",
        "std.boardsAccess.put",
    ] {
        assert!(!may("mem_4", op), "a viewer was granted `{op}`");
        assert!(!may("mem_2", op), "an editor was granted `{op}`");
        assert!(may("mem_o", op), "the owner should hold `{op}`");
    }

    // …and the AUTHORED surface is untouched by the rule, so this test is
    // known to be about std rather than about an empty template.
    assert!(
        may("mem_4", "taskBoard"),
        "authored reads still reach a viewer"
    );
    assert!(
        may("mem_2", "renameTask"),
        "authored writes still reach an editor"
    );
}

/// **The client-facing half of the prune** (5f): a pin says where
/// recomputation still starts, and a prune moves that value while leaving
/// the head — and therefore the signature — exactly where it was.
///
/// A layer-1 pin is not a registered consumer, deliberately: `bootstrap.md`
/// promises the layer "asks nothing of anyone", so the deployment does not
/// know a pin exists and the pruning floor cannot protect one. A client that
/// was away across a prune has lost the ability to recompute forward from
/// its own `upto`, and `x-ordo-verifiable-from` is how it learns that in one
/// request instead of by fetching entries that are gone.
///
/// The second half of this test is the reason the header cannot be an
/// afterthought in the attestation cache: **a prune does not move the
/// head.** A cache keyed on the head alone would keep serving the pre-prune
/// floor — that is, it would go on telling clients that recomputation from
/// genesis is possible on a chain whose prefix has just been deleted, which
/// is the one wrong answer this field exists to prevent.
#[tokio::test(flavor = "multi_thread")]
async fn a_prune_moves_the_pins_floor_though_the_head_stands_still() {
    use ordo_commandlog::{
        Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, SpaceId, Timestamp,
    };

    let db = "ordo_5f_pin_prune";
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db(db);
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let node = boot(&f, &pg, db, false, metrics, None).await;

    let pool = ordo_exec::Pool::connect(cfg(db)).await.expect("pool");
    ordo_exec::prune::ensure_tables(&pool)
        .await
        .expect("tables");
    let space = SpaceId(ordo_host::DEFAULT_SPACE.to_string());
    pool.rows(&format!(
        "DELETE FROM \"$commandlog\" WHERE \"space\" = '{}'",
        ordo_host::DEFAULT_SPACE
    ))
    .await
    .expect("a chain of this test's own");

    // Six groups, stamped long past any horizon this test uses.
    for i in 0..6u64 {
        let tx = pool.begin().await.expect("tx");
        tx.append_group(&CommandGroup {
            correlation: CorrelationId(format!("cg_pin_{i}")),
            actor: Actor::User("usr_1".into()),
            commands: vec![Command {
                id: CommandId(format!("cmd_pin_{i}")),
                space: space.clone(),
                entity: "task".into(),
                row_id: format!("tsk_pin_{i}"),
                kind: CommandKind::Create,
                before: None,
                after: Some(
                    [(
                        "title".to_string(),
                        ordo_commandlog::Value::Text(format!("t{i}")),
                    )]
                    .into_iter()
                    .collect(),
                ),
                at: Timestamp(1_000 + i),
            }],
            owner_administration: false,
            restored_unverified: false,
            restore: None,
        })
        .await
        .expect("append");
        tx.commit().await.expect("commit");
    }

    let (st, h) = get_headers(node.router(), "/op/openTasks", Some("member:mem_1")).await;
    assert_eq!(st, 200);
    let position = hdr(&h, "x-ordo-position").expect("a position");
    let head = hdr(&h, "x-ordo-head").expect("a head");
    let sig = hdr(&h, "x-ordo-head-sig").expect("a signature");
    assert_eq!(
        hdr(&h, "x-ordo-verifiable-from").as_deref(),
        Some("0"),
        "nothing has been pruned, so recomputation still starts at genesis"
    );

    // Sign a checkpoint below the head and prune to it. The signer is the
    // deployment's own — the same one the pin is signed with.
    let signer = ordo_commandlog::sign::Ed25519Signer::from_seed("deployment-test", &[7u8; 32]);
    let head_at_2 = {
        let rows = pool
            .rows(&format!(
                "SELECT encode(\"hash\",'hex') FROM \"$commandlog\" \
                 WHERE \"space\" = '{}' AND \"idx\" = 2",
                ordo_host::DEFAULT_SPACE
            ))
            .await
            .unwrap();
        parse_head(rows[0][0].as_deref().unwrap())
    };
    let at = Timestamp(1_100);
    let cp = ordo_commandlog::Checkpoint {
        space: space.clone(),
        upto: 3,
        head: head_at_2,
        at,
        key_id: ordo_commandlog::Signer::key_id(&signer).to_string(),
        signature: ordo_commandlog::Signer::sign(
            &signer,
            &ordo_commandlog::Checkpoint::signed_bytes(&space, 3, head_at_2, at),
        ),
    };
    let tx = pool.begin().await.unwrap();
    tx.put_checkpoint(&cp).await.unwrap();
    tx.commit().await.unwrap();

    let outcome = ordo_exec::prune::prune(
        &pool,
        &space,
        &signer.verifier(),
        &ordo_exec::cadence::LogPolicy {
            undo_horizon: std::time::Duration::from_secs(10),
            ..ordo_exec::cadence::LogPolicy::DEFAULT
        },
        9_999_999,
    )
    .await
    .expect("lawful prune");
    assert!(
        matches!(
            outcome,
            ordo_exec::prune::Pruned::Removed {
                verifiable_from: 3,
                ..
            }
        ),
        "{outcome:?}"
    );

    let (_, h2) = get_headers(node.router(), "/op/openTasks", Some("member:mem_1")).await;
    assert_eq!(
        hdr(&h2, "x-ordo-verifiable-from").as_deref(),
        Some("3"),
        "the response must say where recomputation now starts"
    );
    // Everything the signature covers is unchanged. This is the property
    // that makes the floor a separate, unsigned header rather than something
    // folded into the pin: the pin did not move, and the client's ability to
    // use it did.
    assert_eq!(hdr(&h2, "x-ordo-position"), Some(position));
    assert_eq!(hdr(&h2, "x-ordo-head"), Some(head));
    assert_eq!(hdr(&h2, "x-ordo-head-sig"), Some(sig));
}

/// C4b's guard-change case (`plan-replica-feed.md` §2): a membership write
/// hides rows a device synced without touching those rows, and the feed
/// sends them as deletes. Real Postgres and the LISTEN bus, because what
/// makes it work is DL G16: an op's invalidation set includes the entities
/// its guards walk.
#[tokio::test(flavor = "multi_thread")]
async fn a_guard_change_reaches_a_synced_device_as_deletes() {
    use axum::body::Body;
    use axum::http::Request;
    use futures::StreamExt;
    use serde_json::{json, Value};
    use std::time::Duration;
    use tower::util::ServiceExt;

    const OPS: &str = r#"
op dropMembership($pm: id) =
  del projectMembers(id == $pm)
"#;
    let f = Arc::new(
        Fixture::from_source(
            &format!("{}\n{OPS}", ordo_conformance::FIXTURE),
            ordo_conformance::SEED,
        )
        .expect("this test's op registers"),
    );
    let pg = make_db("ordo_host_sync_guard");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let node = boot(&f, &pg, "ordo_host_sync_guard", true, metrics, None).await;

    // Every complete frame of one kind; an `error` frame fails the test.
    let frames = |buffer: &str, kind: &str| -> Vec<Value> {
        buffer
            .split("\n\n")
            .filter_map(|frame| {
                let mut event = "";
                let mut data = None;
                for line in frame.lines() {
                    if let Some(e) = line.strip_prefix("event:") {
                        event = e.trim();
                    } else if let Some(d) = line.strip_prefix("data:") {
                        data = serde_json::from_str::<Value>(d.trim()).ok();
                    }
                }
                assert_ne!(event, "error", "the feed failed: {frame}");
                (event == kind).then_some(data).flatten()
            })
            .collect()
    };

    // mem_2 belongs to proj_1 (pm_2) and proj_2 (pm_3), so it reads task_1
    // to task_4 and not proj_3's task_5.
    let resp = node
        .router()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/op/std.tasks.list?live=1&sync=1")
                .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
                .header("authorization", "Bearer member:mem_2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "the sync was refused");
    let mut stream = resp.into_body().into_data_stream();
    let mut buffer = String::new();
    while !frames(&buffer, "load")
        .iter()
        .any(|d| d["complete"] == true)
    {
        let chunk = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("the load arrives")
            .expect("stream open")
            .expect("chunk ok");
        buffer.push_str(&String::from_utf8_lossy(&chunk));
    }
    let mut held: Vec<String> = frames(&buffer, "load")
        .iter()
        .flat_map(|d| d["rows"].as_array().cloned().unwrap_or_default())
        .map(|r| r["id"].as_str().expect("rows carry their id").to_string())
        .collect();
    held.sort();
    assert_eq!(held, ["task_1", "task_2", "task_3", "task_4"]);

    // proj_1's owner removes mem_2 from it. No task row is written.
    let resp = node
        .router()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/op/dropMembership?pm=pm_2")
                .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
                .header("authorization", "Bearer member:mem_1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));

    while frames(&buffer, "change").is_empty() {
        let chunk = tokio::time::timeout(Duration::from_secs(20), stream.next())
            .await
            .expect("the guard change reaches the feed")
            .expect("stream open")
            .expect("chunk ok");
        buffer.push_str(&String::from_utf8_lossy(&chunk));
    }
    let change = &frames(&buffer, "change")[0];
    assert_eq!(change["deletes"], json!(["task_1", "task_2"]));
    assert_eq!(change["upserts"], json!([]));
}

/// C4e: a list that never projected its id is syncable, and the feed keys its
/// rows by the `$id` the executor attaches (`ui-gen.md` §1).
///
/// Against a real database rather than a fake, because the attach reads a
/// column the author did not write and the failure mode is silent: key the
/// rows by a sort value instead and the difference pass sees churn, not an
/// error. `std.tasks.list` cannot prove this — it projects its own id.
#[tokio::test]
async fn a_list_that_never_projected_its_id_syncs_by_the_attached_key() {
    use axum::body::Body;
    use axum::http::Request;
    use futures::StreamExt;
    use serde_json::Value;
    use std::time::Duration;
    use tower::util::ServiceExt;

    const OPS: &str = r#"
op openTitles =
  tasks(status == "open")[title]
"#;
    let f = Arc::new(
        Fixture::from_source(
            &format!("{}\n{OPS}", ordo_conformance::FIXTURE),
            ordo_conformance::SEED,
        )
        .expect("this test's op registers"),
    );
    let pg = make_db("ordo_host_sync_attached");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let node = boot(&f, &pg, "ordo_host_sync_attached", true, metrics, None).await;

    let frames = |buffer: &str, kind: &str| -> Vec<Value> {
        buffer
            .split("\n\n")
            .filter_map(|frame| {
                let mut event = "";
                let mut data = None;
                for line in frame.lines() {
                    if let Some(e) = line.strip_prefix("event:") {
                        event = e.trim();
                    } else if let Some(d) = line.strip_prefix("data:") {
                        data = serde_json::from_str::<Value>(d.trim()).ok();
                    }
                }
                assert_ne!(event, "error", "the feed failed: {frame}");
                data.filter(|_| event == kind)
            })
            .collect()
    };

    let resp = node
        .router()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/op/openTitles?live=1&sync=1")
                .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
                .header("authorization", "Bearer member:mem_1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "an unprojected id is no longer a refusal"
    );

    let mut stream = resp.into_body().into_data_stream();
    let mut buffer = String::new();
    while !frames(&buffer, "load")
        .iter()
        .any(|d| d["complete"] == true)
    {
        let chunk = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("the load arrives")
            .expect("stream open")
            .expect("chunk ok");
        buffer.push_str(&String::from_utf8_lossy(&chunk));
    }

    let loads = frames(&buffer, "load");
    assert_eq!(loads[0]["key"], "$id", "the feed says what it keys rows by");
    assert_eq!(loads[0]["entity"], "tasks");

    let rows: Vec<Value> = loads
        .iter()
        .flat_map(|d| d["rows"].as_array().cloned().unwrap_or_default())
        .collect();
    assert!(!rows.is_empty(), "the seed has open tasks");
    for row in &rows {
        // The identity is a task id, not the `title` this op selects and not
        // a sort value: that is the whole point of the attach.
        let id = row["$id"].as_str().expect("every row carries `$id`");
        assert!(id.starts_with("task_"), "a typed task id, got {id:?}");
        assert!(row["title"].is_string(), "the author's projection survives");
    }
    let unique: std::collections::BTreeSet<&str> =
        rows.iter().map(|r| r["$id"].as_str().unwrap()).collect();
    assert_eq!(unique.len(), rows.len(), "one row, one id");
}

/// R6 (`plan-refinement.md` section 7, ruled 2026-10-05 ask 2): a sync takes
/// a refinement's `where`. A row that stops matching leaves the result and
/// is a `delete`; a row that starts matching is an `upsert`. Both fall out of
/// the existing diff, over real Postgres and the LISTEN bus. The single-read
/// live stream's first frame is the refined read.
#[tokio::test(flavor = "multi_thread")]
async fn a_refined_sync_sends_rows_entering_and_leaving_the_filter() {
    use axum::body::Body;
    use axum::http::Request;
    use futures::StreamExt;
    use serde_json::{json, Value};
    use std::time::Duration;
    use tower::util::ServiceExt;

    const OPS: &str = r#"
op reopenTask($task: id) =
  set tasks(id == $task) { status: "open" }
"#;
    let f = Arc::new(
        Fixture::from_source(
            &format!("{}\n{OPS}", ordo_conformance::FIXTURE),
            ordo_conformance::SEED,
        )
        .expect("this test's op registers"),
    );
    let pg = make_db("ordo_host_sync_refined");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let node = boot(&f, &pg, "ordo_host_sync_refined", true, metrics, None).await;

    let frames = |buffer: &str, kind: &str| -> Vec<Value> {
        buffer
            .split("\n\n")
            .filter_map(|frame| {
                let mut event = "";
                let mut data = None;
                for line in frame.lines() {
                    if let Some(e) = line.strip_prefix("event:") {
                        event = e.trim();
                    } else if let Some(d) = line.strip_prefix("data:") {
                        data = serde_json::from_str::<Value>(d.trim()).ok();
                    }
                }
                assert_ne!(event, "error", "the feed failed: {frame}");
                data.filter(|_| event == kind || (kind == "message" && event.is_empty()))
            })
            .collect()
    };
    let refine = |r: Value| -> String {
        r.to_string()
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                other => format!("%{other:02X}"),
            })
            .collect()
    };
    let get = |uri: String| {
        Request::builder()
            .method("GET")
            .uri(uri)
            .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
            .header("authorization", "Bearer member:mem_1")
            .body(Body::empty())
            .unwrap()
    };
    let open = refine(json!({"where": [{"field": "status", "op": "eq", "value": "open"}]}));

    // A sync with a pick is refused by class, naming `pick`.
    let resp = node
        .router()
        .oneshot(get(format!(
            "/op/std.tasks.list?live=1&sync=1&$refine={}",
            refine(json!({"pick": ["title"]}))
        )))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["error"]["code"], "invalid_refinement", "{body}");
    assert_eq!(body["error"]["param"], "pick", "{body}");

    // The single-read live stream: its first frame is the refined read.
    let resp = node
        .router()
        .oneshot(get(format!("/op/std.tasks.list?live=1&$refine={open}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "a refined live read is accepted");
    let mut live = resp.into_body().into_data_stream();
    let mut first = String::new();
    while frames(&first, "message").is_empty() {
        let chunk = tokio::time::timeout(Duration::from_secs(10), live.next())
            .await
            .expect("the first frame arrives")
            .expect("stream open")
            .expect("chunk ok");
        first.push_str(&String::from_utf8_lossy(&chunk));
    }
    let shown = frames(&first, "message")[0]["rows"]
        .as_array()
        .cloned()
        .expect("the envelope's rows");
    assert!(!shown.is_empty());
    assert!(shown.iter().all(|r| r["status"] == "open"), "{shown:?}");
    drop(live);

    // The refined sync.
    let resp = node
        .router()
        .oneshot(get(format!(
            "/op/std.tasks.list?live=1&sync=1&$refine={open}"
        )))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "a sync with `where` is accepted");
    let mut stream = resp.into_body().into_data_stream();
    let mut buffer = String::new();
    while !frames(&buffer, "load")
        .iter()
        .any(|d| d["complete"] == true)
    {
        let chunk = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("the load arrives")
            .expect("stream open")
            .expect("chunk ok");
        buffer.push_str(&String::from_utf8_lossy(&chunk));
    }
    let loaded: Vec<Value> = frames(&buffer, "load")
        .iter()
        .flat_map(|d| d["rows"].as_array().cloned().unwrap_or_default())
        .collect();
    let ids: Vec<&str> = loaded.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"task_1"), "{ids:?}");
    assert!(!ids.contains(&"task_2"), "task_2 is done: {ids:?}");
    assert!(loaded.iter().all(|r| r["status"] == "open"));

    let write = |op: &str, task: &str| {
        Request::builder()
            .method("POST")
            .uri(format!("/op/{op}"))
            .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
            .header("authorization", "Bearer member:mem_1")
            .header("content-type", "application/json")
            .body(Body::from(json!({ "task": task }).to_string()))
            .unwrap()
    };
    // The `n`-th change frame, read off the feed as it arrives.
    macro_rules! next_change {
        ($n:expr) => {{
            while frames(&buffer, "change").len() < $n {
                let chunk = tokio::time::timeout(Duration::from_secs(20), stream.next())
                    .await
                    .expect("the change reaches the feed")
                    .expect("stream open")
                    .expect("chunk ok");
                buffer.push_str(&String::from_utf8_lossy(&chunk));
            }
            frames(&buffer, "change")[$n - 1].clone()
        }};
    }

    // task_1 is closed: it leaves the filter, and the feed deletes it.
    let resp = node
        .router()
        .oneshot(write("closeTask", "task_1"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let change = next_change!(1);
    assert_eq!(change["deletes"], json!(["task_1"]));
    assert_eq!(change["upserts"], json!([]));

    // task_2 is reopened: it enters the filter, and the feed upserts it.
    let resp = node
        .router()
        .oneshot(write("reopenTask", "task_2"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let change = next_change!(2);
    assert_eq!(change["deletes"], json!([]));
    let upserts = change["upserts"].as_array().cloned().unwrap();
    assert_eq!(upserts.len(), 1, "{upserts:?}");
    assert_eq!(upserts[0]["id"], "task_2");
    assert_eq!(upserts[0]["status"], "open");
}

// ── 7d(b): the owner's view of their own space's security record ──────────
//
// `security-logging-design.md` §6.1 routes a space's own denials to that
// space's stream **precisely so its owner can see them**, and until this
// route existed no owner could see them at all: `/security/alerts` serves
// the deny-volume monitor's firings, which is the platform's judgement about
// who looks like they are probing, not the record itself.
//
// The gate is what makes or breaks this. Every other route on the platform
// router authorizes against the *deployment's* own space, because every
// other route is an operator surface. Reusing that gate here would produce a
// route that returns data, passes a naive test, and still leaves §6.1's
// members-only routing pointing at readers who cannot read.

/// A deployment whose `/security/events` can find a space's stream: one
/// database, which is `SinglePool`'s whole meaning and the ordinary small
/// deployment.
fn with_single_pool(node: &mut Node, pool: ordo_exec::Pool) {
    node.platform = node
        .platform
        .clone()
        .with_pools(std::sync::Arc::new(ordo_exec::route::SinglePool::new(pool)));
}

async fn seed_denials(pool: &ordo_exec::Pool, space: &str, n: u32) {
    let store =
        ordo_exec::secevent::PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    for i in 0..n {
        let entry = ordo_secevent::StreamEntry::single(
            ordo_secevent::Scope::Space(ordo_commandlog::SpaceId(space.to_string())),
            ordo_secevent::Subject::member("prober"),
            "openTasks",
            ordo_secevent::EventKind::denied(ordo_secevent::RecordedDecision::NoMembership),
            ordo_commandlog::Timestamp(3_000 + u64::from(i)),
            ordo_commandlog::CorrelationId(format!("cor_{i:08x}")),
        );
        store.try_append(&entry).await.expect("security append");
    }
}

/// **A space owner reads their own space's record, and nobody else's.**
///
/// The one property this feature is for, and the one an operator-scoped gate
/// would silently invert.
#[tokio::test(flavor = "multi_thread")]
async fn the_events_route_answers_a_space_owner_for_their_own_space_only() {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    use ordo_identity::{TokenKey, TokenSpec};

    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_host_secevents_route");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let mut node = boot(
        &f,
        &pg,
        "ordo_host_secevents_route",
        false,
        metrics.clone(),
        None,
    )
    .await;
    let pool = ordo_exec::Pool::connect(cfg("ordo_host_secevents_route"))
        .await
        .expect("pool");
    pool.rows("DELETE FROM \"$secevents\"").await.unwrap();
    with_single_pool(&mut node, pool.clone());

    let key = TokenKey::from_secret(b"secevents-route-test");
    let mut engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    let (other_space, intruder) = add_intruder_space(&mut engine);
    let access = Arc::new(ordo_host::IdentityAccess {
        engine: std::sync::Arc::new(engine),
        key: TokenKey::from_secret(b"secevents-route-test"),
        now: Box::new(|| 1_000),
        stream: Arc::new(ordo_secevent::Recorder::default()),
        metrics: metrics.clone(),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });
    node.platform.access = access.clone();
    node.api.access = access;
    let token = |id: &str| {
        ordo_identity::mint(
            &key,
            &TokenSpec {
                principal: PrincipalId(id.into()),
                expires: Timestamp(9_999),
                client: None,
                user_presence_at: None,
                capability: None,
                delegation: None,
                session: None,
            },
        )
    };

    // Two spaces, two records. The intruder's own space has three denials in
    // it; the deployment's own has one, and the numbers differ so a test
    // that read the wrong stream cannot pass by coincidence.
    seed_denials(&pool, &other_space, 3).await;
    seed_denials(&pool, ordo_host::DEFAULT_SPACE, 1).await;

    // Public: refused, the same shape as every other refusal here.
    let (st, body) = get(
        node.router(),
        &format!("/security/events?space={other_space}"),
        None,
    )
    .await;
    assert_eq!(st, 403, "the events route must not be public: {body}");

    // **The owner of `spc_intruder` reads `spc_intruder`.** This is the
    // clause that fails if the gate binds the deployment's space.
    let (st, body) = get(
        node.router(),
        &format!("/security/events?space={other_space}"),
        Some(&token(&intruder)),
    )
    .await;
    assert_eq!(
        st, 200,
        "a space owner could not read their own space's security record: {body}"
    );
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["space"], other_space, "{body}");
    assert_eq!(
        v["events"].as_array().map(|a| a.len()),
        Some(3),
        "the wrong space's stream was read: {body}"
    );
    assert_eq!(v["events"][0]["subject"], "user:prober", "{body}");
    assert_eq!(v["events"][0]["kind"], "denied", "{body}");
    // Reported on every page, so "no denials" and "the denials there were
    // have aged out" are distinguishable.
    assert_eq!(v["verifiableFrom"], 0, "{body}");
    assert_eq!(v["next"], 3, "{body}");

    // **And not the deployment's own space**, where they hold nothing.
    let (st, body) = get(
        node.router(),
        &format!("/security/events?space={}", ordo_host::DEFAULT_SPACE),
        Some(&token(&intruder)),
    )
    .await;
    assert_eq!(
        st, 403,
        "a tenant owner read another space's security record: {body}"
    );

    // The deployment owner reads the deployment's own space the same way —
    // through membership, not through being an operator.
    let (st, body) = get(
        node.router(),
        &format!("/security/events?space={}", ordo_host::DEFAULT_SPACE),
        Some(&token("mem_o")),
    )
    .await;
    assert_eq!(st, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["events"].as_array().map(|a| a.len()), Some(1), "{body}");

    // A non-owner member of that space: refused. `mem_1` is a manager.
    let (st, body) = get(
        node.router(),
        &format!("/security/events?space={}", ordo_host::DEFAULT_SPACE),
        Some(&token("mem_1")),
    )
    .await;
    assert_eq!(st, 403, "a manager read the security record: {body}");
}

/// **The deployment scope is not reachable through this route.**
///
/// `$deployment` is where everything with no space relationship lands —
/// unverified callers, non-members, tokens naming a space they have nothing
/// to do with — and it is the only place a **cross-space sweep** is visible.
/// §6.1 keeps it from tenants on purpose. It is not a valid space id, so it
/// cannot arrive through `well_formed_space`; it is refused by name anyway,
/// because a disclosure that size should not rest on another function's
/// alphabet.
#[tokio::test(flavor = "multi_thread")]
async fn the_events_route_refuses_the_deployment_scope_and_malformed_ids() {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    use ordo_identity::{TokenKey, TokenSpec};

    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_host_secevents_scope");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let mut node = boot(
        &f,
        &pg,
        "ordo_host_secevents_scope",
        false,
        metrics.clone(),
        None,
    )
    .await;
    let pool = ordo_exec::Pool::connect(cfg("ordo_host_secevents_scope"))
        .await
        .expect("pool");
    pool.rows("DELETE FROM \"$secevents\"").await.unwrap();
    with_single_pool(&mut node, pool.clone());

    let key = TokenKey::from_secret(b"secevents-scope-test");
    let engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    let access = Arc::new(ordo_host::IdentityAccess {
        engine: std::sync::Arc::new(engine),
        key: TokenKey::from_secret(b"secevents-scope-test"),
        now: Box::new(|| 1_000),
        stream: Arc::new(ordo_secevent::Recorder::default()),
        metrics: metrics.clone(),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });
    node.platform.access = access.clone();
    node.api.access = access;
    let owner = ordo_identity::mint(
        &key,
        &TokenSpec {
            principal: PrincipalId("mem_o".into()),
            expires: Timestamp(9_999),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    );

    // There ARE deployment-scope entries, so a route that served them would
    // return a non-empty body rather than an empty one that looks refused.
    let store =
        ordo_exec::secevent::PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    store
        .try_append(&ordo_secevent::StreamEntry::single(
            ordo_secevent::Scope::Deployment,
            ordo_secevent::Subject::Unverified,
            "openTasks",
            ordo_secevent::EventKind::denied(ordo_secevent::RecordedDecision::TokenUnverifiable),
            ordo_commandlog::Timestamp(4_000),
            ordo_commandlog::CorrelationId("cor_dep".into()),
        ))
        .await
        .expect("deployment-scope append");

    // Each of these is legal in a URI query value and illegal as a space id,
    // so the route is what refuses them. (A probe with a space or a slash in
    // it never reaches the route at all: the request builder rejects the URI,
    // which the first version of this test discovered by panicking in the
    // helper rather than by asserting anything.)
    for probe in [
        ordo_secevent::DEPLOYMENT_KEY,
        "spc.dot",
        "spc:colon",
        "spc$dollar",
    ] {
        let (st, body) = get(
            node.router(),
            &format!("/security/events?space={probe}"),
            Some(&owner),
        )
        .await;
        assert_eq!(
            st, 403,
            "`{probe}` was not refused, and the deployment stream is the one \
             artifact tenants must not read: {body}"
        );
        assert!(
            !body.contains("cor_dep"),
            "`{probe}` leaked a deployment-scope entry: {body}"
        );
    }

    // `space` missing at all is a usage error, not a denial: it says nothing
    // about what exists.
    let (st, body) = get(node.router(), "/security/events", Some(&owner)).await;
    assert_eq!(st, 400, "{body}");

    // **Now isolate the guard, because the loop above does not.**
    //
    // Deleting the `$deployment` check from the route leaves that loop
    // passing, which was found by breaking it: the access check refuses
    // `$deployment` on its own, since nobody holds a membership in a space
    // by that name. So the loop proves the refusal and not the guard, and
    // the guard earns its place only in the case where that assumption does
    // not hold.
    //
    // That case is reachable. `provision` would never write it, but a
    // membership row is a row: a hand-written INSERT, an import, or a bug in
    // something that derives `$space` could put one there. Then the access
    // check **allows**, and the explicit refusal is the only thing standing
    // between a tenant and the cross-space sweep §6.1 keeps from them.
    let mut engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    {
        use ordo_identity::principal::{AccessProfile, PrincipalId, Role, SpaceId};
        let scope = SpaceId(ordo_secevent::DEPLOYMENT_KEY.to_string());
        let sneak = PrincipalId("sneak".to_string());
        engine.set_membership(&scope, &sneak, Role::Owner);
        engine.set_profile(
            &scope,
            AccessProfile::new().allow(Role::Owner, ordo_host::serve::SECEVENTS_OP),
        );
    }
    let access = Arc::new(ordo_host::IdentityAccess {
        engine: std::sync::Arc::new(engine),
        key: TokenKey::from_secret(b"secevents-scope-test"),
        now: Box::new(|| 1_000),
        stream: Arc::new(ordo_secevent::Recorder::default()),
        metrics: metrics.clone(),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });
    node.platform.access = access.clone();
    node.api.access = access;
    let sneak_token = ordo_identity::mint(
        &key,
        &TokenSpec {
            principal: PrincipalId("sneak".into()),
            expires: Timestamp(9_999),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    );
    let (st, body) = get(
        node.router(),
        &format!("/security/events?space={}", ordo_secevent::DEPLOYMENT_KEY),
        Some(&sneak_token),
    )
    .await;
    assert_eq!(
        st, 403,
        "a membership row naming the deployment scope opened the cross-space          sweep to whoever held it: {body}"
    );
    assert!(
        !body.contains("cor_dep"),
        "the deployment stream leaked to a membership in its own storage key: {body}"
    );
}

/// **No resolver is an error, never an empty page.**
///
/// S4 routes each space's entries to that space's own database. A process
/// with no resolver reading from the control pool would find nothing for a
/// routed tenant, and for this surface "nothing" renders as *your space has
/// recorded no denials*. A clean bill of health for a stream nobody looked at
/// is the worst answer this route could give.
#[tokio::test(flavor = "multi_thread")]
async fn the_events_route_refuses_rather_than_reporting_a_stream_it_cannot_locate() {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    use ordo_identity::{TokenKey, TokenSpec};

    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = make_db("ordo_host_secevents_unwired");
    f.reset(&pg).expect("seeded");
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let mut node = boot(
        &f,
        &pg,
        "ordo_host_secevents_unwired",
        false,
        metrics.clone(),
        None,
    )
    .await;
    // Deliberately NOT wiring a resolver.
    assert!(
        node.platform.pools.is_none(),
        "this test is about the unwired case"
    );

    let key = TokenKey::from_secret(b"secevents-unwired-test");
    let engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    let access = Arc::new(ordo_host::IdentityAccess {
        engine: std::sync::Arc::new(engine),
        key: TokenKey::from_secret(b"secevents-unwired-test"),
        now: Box::new(|| 1_000),
        stream: Arc::new(ordo_secevent::Recorder::default()),
        metrics: metrics.clone(),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });
    node.platform.access = access.clone();
    node.api.access = access;
    let owner = ordo_identity::mint(
        &key,
        &TokenSpec {
            principal: PrincipalId("mem_o".into()),
            expires: Timestamp(9_999),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    );

    let (st, body) = get(
        node.router(),
        &format!("/security/events?space={}", ordo_host::DEFAULT_SPACE),
        Some(&owner),
    )
    .await;
    assert_eq!(
        st, 503,
        "an unlocatable stream answered as though it were empty: {body}"
    );
    assert!(body.contains("space_unavailable"), "{body}");
    assert!(
        !body.contains("\"events\""),
        "a refusal carried an events array: {body}"
    );
}
