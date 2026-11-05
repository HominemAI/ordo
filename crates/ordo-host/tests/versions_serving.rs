//! Increment 3 of `plan-versions.md`, session 1: each tenant is served the
//! version it is pinned to (§1.3, §1.4, asks 16 and 17).
//!
//! - One door, one node, 2 tenants on 2 versions: each is served its own,
//!   with no refusal, and the op's shape is its own version's.
//! - A write under a stale pin is refused by the pin fence in the tenant's
//!   database; the door reads the pin again and the caller sees nothing.
//! - A tenant pinned to a version the node has not loaded, or whose
//!   database is not that version's shape, is closed with a plain answer,
//!   and the deployment owner's reason names the fix.
//! - The pin stream: a pin moved by `Versions::pin` reaches a node by its
//!   stream, which loads the version and drops it when no pin names it.
//! - Which moves wait for a maintenance window: none yet. An activation and
//!   a pin move a tenant at once; a tenant on a revoked version stays.
//!
//! A regression looks like a tenant answered from the active version's
//! schema, a write landing under a pin another node already moved past, or
//! a version loaded because a request asked for it.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op;
use ordo_host::deploy::{self, Deployment, PinMap, Swapped};
use ordo_host::schema_apply::Program;
use ordo_host::spaces::Spaces;
use ordo_host::tenants;
use ordo_host::versions::{VersionRefusal, Versions};
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"host-versions-serving-secret";
const HERE: &str = ordo_host::DEFAULT_SPACE;
const TWIN: &str = "spc_twin";
const OWNER: &str = "mem_o";
const TWIN_OWNER: &str = "mem_twin";

/// Version 1 of a notes entity. `jot` is declared the same way in v1 and
/// v2; only v2's writes a body, so a row says which version wrote it.
const NOTES_V1: &str = "
entity memos (key: memo) {
  owner -> members
  title:  text
  access read   owner == $caller
  access write  owner == $caller
}

op jot($owner: text, $title: text) =
  set memos { owner: $owner, title: $title }
";

/// Version 2: an optional field added. Additive, so one database serves
/// both.
const NOTES_V2: &str = "
entity memos (key: memo) {
  owner -> members
  title:  text
  body:   text?
  access read   owner == $caller
  access write  owner == $caller
}

op jot($owner: text, $title: text) =
  set memos { owner: $owner, title: $title, body: \"from v2\" }
";

/// Version 3: an entity the database has no table for.
const NOTES_V3: &str = "
entity memos (key: memo) {
  owner -> members
  title:  text
  body:   text?
  access read   owner == $caller
  access write  owner == $caller
}

entity tags {
  name: text
}
";

struct Harness {
    app: axum::Router,
    pool: Pool,
    deployment: Arc<Deployment>,
    spaces: Arc<Spaces>,
}

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        ..PoolConfig::from_env()
    }
}

/// One node: the active version is v2, and v1 is served beside it. The
/// database is v2's shape. `TWIN` is provisioned like `HERE`.
async fn harness(db: &str) -> Harness {
    let f2 = Arc::new(Fixture::from_source(&format!("{FIXTURE}\n{NOTES_V2}"), SEED).expect("v2"));
    let pg = Pg::named(db);
    f2.reset(&pg).expect("seeded");
    let pool = Pool::connect(cfg(db)).await.expect("pool");
    let deployment = Deployment::pinned(
        "v2",
        f2.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f2.schema.clone(),
            registry: f2.registry.clone(),
            keys: f2.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    let f1 = Fixture::from_source(&format!("{FIXTURE}\n{NOTES_V1}"), SEED).expect("v1");
    let v1 = deploy::verify_embedded_pinned(
        f1,
        &pool,
        &cfg(db),
        f2.keys.clone(),
        ordo_conformance::fixture_space(),
        "v1",
    )
    .await
    .expect("v1 gates beside v2, its database checked per tenant");
    assert!(matches!(deployment.install(v1), Swapped::Installed { .. }));

    ordo_host::provision::provision_space(
        &pool,
        f2.keys.as_ref(),
        TWIN,
        &ordo_host::provision::FoundingOwner {
            member_id: TWIN_OWNER,
            principal: TWIN_OWNER,
            display_name: "The Twin's owner",
        },
    )
    .await
    .expect("the twin provisions");
    let engine = ordo_host::deployment_engine(&f2, &pg).expect("engine");
    let access = Arc::new(ordo_host::IdentityAccess {
        engine: Arc::new(engine),
        key: TokenKey::from_secret(SECRET),
        now: Box::new(|| NOW),
        stream: Arc::new(ordo_secevent::Recorder::default()),
        metrics: Arc::new(ordo_host::metrics::Metrics::new()),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });
    let spaces = Spaces::routed(
        deployment.clone(),
        Arc::new(SinglePool::new(pool.clone())),
        ordo_exec::witness::Witness::new(pool.clone()),
    );
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(ordo_host::FixtureExecutor {
            spaces: spaces.clone(),
            pg: pg.clone(),
            page_size: 50,
        }),
        access,
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"host-versions-serving"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        app: router(state),
        pool,
        deployment,
        spaces,
    }
}

fn token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

async fn call(
    h: &Harness,
    method: Method,
    op: &str,
    space: &str,
    who: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(format!("/op/{op}"))
        .header("authorization", format!("Bearer {}", token(who)))
        .header("x-ordo-space", space);
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let resp = h
        .app
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn create(h: &Harness, space: &str, who: &str, fields: Value) -> (u16, Value) {
    call(
        h,
        Method::POST,
        "std.memos.create",
        space,
        who,
        Some(fields),
    )
    .await
}

async fn list(h: &Harness, space: &str, who: &str) -> Vec<Value> {
    let (st, body) = call(h, Method::GET, "std.memos.list", space, who, None).await;
    assert_eq!(st, 200, "{space} reads its notes: {body}");
    body["rows"].as_array().cloned().unwrap_or_default()
}

/// **The done-when** (§4 row 3): one door, 2 tenants on 2 versions, and
/// neither is refused. Each is answered in its own version's shape.
#[tokio::test(flavor = "multi_thread")]
async fn two_tenants_on_two_versions_are_each_served_their_own() {
    let h = harness("ordo_vserve_two").await;
    h.deployment.note_pin(HERE, "v2", 1, 0);
    h.deployment.note_pin(TWIN, "v1", 1, 0);

    let (st, body) = create(
        &h,
        HERE,
        OWNER,
        json!({"owner": OWNER, "title": "here", "body": "v2 has it"}),
    )
    .await;
    assert_eq!(st, 200, "v2's tenant writes a body: {body}");
    let (st, body) = create(
        &h,
        TWIN,
        TWIN_OWNER,
        json!({"owner": TWIN_OWNER, "title": "twin"}),
    )
    .await;
    assert_eq!(st, 200, "v1's tenant writes, no refusal: {body}");

    let here = list(&h, HERE, OWNER).await;
    assert_eq!(here.len(), 1);
    assert_eq!(here[0]["body"], "v2 has it", "{here:?}");
    let twin = list(&h, TWIN, TWIN_OWNER).await;
    assert_eq!(twin.len(), 1);
    assert_eq!(twin[0]["title"], "twin");
    assert!(
        twin[0].get("body").is_none(),
        "the v1 tenant is answered in v1's shape, which has no body: {twin:?}"
    );

    // The op is the tenant's version's: v1's create takes no body.
    let (st, body) = create(
        &h,
        TWIN,
        TWIN_OWNER,
        json!({"owner": TWIN_OWNER, "title": "t", "body": "x"}),
    )
    .await;
    assert_eq!(st, 400, "v1's create has no `body`: {body}");
}

/// **The pin fence** (§1.3, ask 16): another node already wrote this
/// tenant under a newer pin, to another version. This node's write is
/// refused in the tenant's own database, the door reads the pin again and
/// writes once more **under the new version**, and the caller sees one
/// success.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_under_a_stale_pin_is_fenced_and_retried_on_the_new_pin() {
    let h = harness("ordo_vserve_fence").await;
    tenants::ensure_table(&h.pool).await.expect("$spaces");
    assert!(tenants::reserve(&h.pool, TWIN, TWIN, "dev:ada", 10, None)
        .await
        .expect("reserved"));
    tenants::activate(&h.pool, TWIN, 11).await.expect("active");
    let control = |version: &str, epoch: i64| {
        format!(
            "UPDATE \"$spaces\" SET \"version\" = '{version}', \"pin_epoch\" = {epoch} \
             WHERE \"$space\" = '{TWIN}'"
        )
    };
    let fence = |epoch: i64| {
        format!(
            "INSERT INTO \"$pin_fence\" (\"space\", \"epoch\") VALUES ('{TWIN}', {epoch}) \
             ON CONFLICT (\"space\") DO UPDATE SET \"epoch\" = EXCLUDED.\"epoch\""
        )
    };
    h.pool
        .rows(&control("v2", 7))
        .await
        .expect("the control database says v2 at epoch 7");
    h.pool
        .rows(&fence(7))
        .await
        .expect("another node wrote under epoch 7");
    // This node still holds v1 at epoch 3.
    h.deployment.note_pin(TWIN, "v1", 3, 0);

    let (st, body) = call(
        &h,
        Method::POST,
        "jot",
        TWIN,
        TWIN_OWNER,
        Some(json!({"owner": TWIN_OWNER, "title": "t"})),
    )
    .await;
    assert_eq!(st, 200, "the caller sees nothing of the stale pin: {body}");
    assert_eq!(
        h.deployment.pin_of(TWIN),
        Some(("v2".to_string(), 7)),
        "the door read the pin again"
    );
    let rows = list(&h, TWIN, TWIN_OWNER).await;
    assert_eq!(rows.len(), 1, "written once: {rows:?}");
    assert_eq!(
        rows[0]["body"], "from v2",
        "the retry ran v2's op, not v1's: {rows:?}"
    );

    // **The retry passes the door again** when the new version declares
    // the op differently: v1's create has no `body`, v2's does. The door
    // bound and checked v1's, so nothing is written and the caller retries.
    h.pool.rows(&control("v2", 9)).await.expect("v2 at 9");
    h.pool.rows(&fence(9)).await.expect("fence at 9");
    h.deployment.note_pin(TWIN, "v1", 8, 0);
    let (st, body) = create(
        &h,
        TWIN,
        TWIN_OWNER,
        json!({"owner": TWIN_OWNER, "title": "u"}),
    )
    .await;
    assert_eq!(st, 503, "{body}");
    assert_eq!(body["error"]["code"], "server_busy", "{body}");
    assert_eq!(list(&h, TWIN, TWIN_OWNER).await.len(), 1, "nothing written");
    assert_eq!(h.deployment.pin_of(TWIN), Some(("v2".to_string(), 9)));
    let (st, body) = create(
        &h,
        TWIN,
        TWIN_OWNER,
        json!({"owner": TWIN_OWNER, "title": "u", "body": "b"}),
    )
    .await;
    assert_eq!(st, 200, "the caller's retry lands under v2: {body}");

    // A node ahead of the fence raises it, in the write's transaction.
    h.deployment.note_pin(TWIN, "v2", 11, 0);
    let (st, body) = create(
        &h,
        TWIN,
        TWIN_OWNER,
        json!({"owner": TWIN_OWNER, "title": "w"}),
    )
    .await;
    assert_eq!(st, 200, "{body}");
    let fence = h
        .pool
        .rows(&format!(
            "SELECT \"epoch\" FROM \"$pin_fence\" WHERE \"space\" = '{TWIN}'"
        ))
        .await
        .expect("fence");
    assert_eq!(fence[0][0].as_deref(), Some("11"));
}

/// **An activation this node has not swapped to yet** (review,
/// 2026-11-05). The registry's active version moved every pin, and this
/// node is still compiling it. A tenant is served the version it was
/// pinned to before, under that pin's epoch, so once a node that has
/// swapped writes under the new epoch, this node's writes are fenced.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_still_loading_an_activation_serves_the_old_pin_under_its_old_epoch() {
    let h = harness("ordo_vserve_following").await;
    // TWIN was on v1 (beside the active v2); the activation names v9,
    // which this node has not loaded.
    h.deployment.note_pin(TWIN, "v1", 3, 0);
    h.deployment.follow(Some("v9".into()));
    h.deployment.note_pin(TWIN, "v9", 4, 0);
    let (loaded, epoch) = h.deployment.for_space(TWIN).expect("served");
    assert_eq!(
        (loaded.version.as_str(), epoch),
        ("v1", Some(3)),
        "the version and epoch it held before, not the active v2 under the new epoch"
    );
    assert!(
        h.deployment.retain_pinned().is_empty(),
        "v1 stays loaded while its tenant waits for the swap"
    );
    // A tenant this node held no earlier pin for waits for the swap.
    h.deployment.note_pin(HERE, "v9", 4, 0);
    assert!(h.deployment.for_space(HERE).is_err());

    let (st, body) = create(
        &h,
        TWIN,
        TWIN_OWNER,
        json!({"owner": TWIN_OWNER, "title": "before"}),
    )
    .await;
    assert_eq!(st, 200, "no node wrote under the new epoch yet: {body}");

    // A node that swapped writes under epoch 4: old code no longer writes.
    h.pool
        .rows(&format!(
            "UPDATE \"$pin_fence\" SET \"epoch\" = 4 WHERE \"space\" = '{TWIN}'"
        ))
        .await
        .expect("fence raised");
    let (st, body) = create(
        &h,
        TWIN,
        TWIN_OWNER,
        json!({"owner": TWIN_OWNER, "title": "after"}),
    )
    .await;
    assert_eq!(st, 503, "{body}");
    assert_eq!(body["error"]["code"], "server_busy", "{body}");
    assert_eq!(list(&h, TWIN, TWIN_OWNER).await.len(), 1, "nothing written");
}

/// **A whole read racing the stream** (review, 2026-11-05): the reconcile
/// tick reads every pin while the nudge's tail takes a newer move and
/// loads its version. The whole read lands last and must not move that
/// pin back, move the read position back, or drop the version.
#[tokio::test(flavor = "multi_thread")]
async fn a_whole_read_racing_the_stream_never_moves_a_pin_back() {
    let (pool, c) = registry_db("ordo_vserve_race").await;
    tenant(&pool, "spc_a").await;
    apply(&pool, V010).await;
    apply(&pool, V020).await;
    let (d, stream) = node(&pool, &c).await;
    stream.reload().await.expect("boot read");

    // The tick begins: it takes the pins, then reads the database.
    let before = d.pins();
    let (rows, seq) = Versions::on(&pool).all_pins().await.expect("whole read");
    // The nudge lands meanwhile.
    let moved = Versions::on(&pool)
        .pin("spc_a", "#1", "deployment-owner:ada", 50)
        .await
        .expect("pinned");
    let tail = stream.tail().await.expect("tail");
    assert_eq!(tail.loaded, vec!["0.1.0".to_string()], "{tail:?}");
    let read_to = d.pins().seq;
    assert!(read_to > seq);

    // The tick takes its read last.
    let mut read = PinMap {
        seq,
        ..PinMap::default()
    };
    for (space, version, epoch) in rows {
        read.by_space.insert(space, (version, epoch));
    }
    d.replace_pins(read, &before);
    assert_eq!(
        d.pin_of("spc_a"),
        Some(("0.1.0".to_string(), moved.epoch)),
        "the newer pin stands"
    );
    assert_eq!(d.pins().seq, read_to, "the read position never moves back");
    assert!(d.retain_pinned().is_empty(), "0.1.0 stays loaded");
    assert_eq!(d.for_space("spc_a").expect("served").0.version, "0.1.0");
}

/// The fence at the executor: a write under an older epoch is refused as
/// `PinMoved`, typed, retryable, and writes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_fence_refuses_an_older_epoch_with_a_typed_error() {
    let h = harness("ordo_vserve_fence_typed").await;
    h.pool
        .rows(&format!(
            "INSERT INTO \"$pin_fence\" (\"space\", \"epoch\") VALUES ('{TWIN}', 5)"
        ))
        .await
        .expect("fence");
    let bound = h.spaces.resolve(TWIN).await.expect("resolves");
    let stale = bound
        .view
        .exec
        .pool
        .for_space(&ordo_crypto::SpaceId(TWIN.into()))
        .with_pin_epoch(4);
    let exec = ordo_exec::Executor {
        pool: stale,
        ..clone_exec(&bound.view.exec)
    };
    let f = bound.view.fixture.clone();
    let err = exec
        .platform_write(bound.log.clone(), move |db, log| {
            f.run_write_on(
                db,
                log,
                &format!("std.memos.create($owner: \"{TWIN_OWNER}\", $title: \"x\")"),
                Some(TWIN_OWNER),
            )
        })
        .await
        .expect_err("refused");
    let pe = err.pool_error().expect("a pool error");
    assert!(
        matches!(
            pe,
            ordo_exec::PoolError::PinMoved {
                held: 4,
                fence: 5,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(pe.class(), ordo_exec::FaultClass::Retryable);
    assert!(err.to_string().contains("pin_moved"), "{err}");
}

fn clone_exec(e: &ordo_exec::Executor) -> ordo_exec::Executor {
    ordo_exec::Executor {
        pool: e.pool.clone(),
        schema: e.schema.clone(),
        registry: e.registry.clone(),
        keys: e.keys.clone(),
        space: e.space.clone(),
    }
}

/// A tenant pinned to a version this node has not loaded is closed for a
/// moment, with no version named to the caller. No request loads it.
#[tokio::test(flavor = "multi_thread")]
async fn a_pin_to_a_version_not_loaded_closes_the_tenant_and_loads_nothing() {
    let h = harness("ordo_vserve_unserved").await;
    h.deployment.note_pin(TWIN, "v9", 1, 0);
    let (st, body) = call(&h, Method::GET, "std.memos.list", TWIN, TWIN_OWNER, None).await;
    assert_eq!(st, 503, "{body}");
    assert_eq!(body["error"]["code"], "space_unavailable", "{body}");
    assert!(
        !body.to_string().contains("v9"),
        "the caller learns no version: {body}"
    );
    assert!(
        h.deployment.loaded("v9").is_none(),
        "a request never loads a version"
    );
    // The other tenant is served on.
    assert_eq!(list(&h, HERE, OWNER).await.len(), 0);
}

/// **The shape fingerprint** (§1.4 step 6): a tenant pinned to a version
/// its database does not hold the shape of is closed, and the reason the
/// deployment owner reads names what is missing and the fix.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_not_in_its_versions_shape_is_closed_naming_the_fix() {
    let h = harness("ordo_vserve_unfit").await;
    let f3 = Fixture::from_source(&format!("{FIXTURE}\n{NOTES_V3}"), SEED).expect("v3");
    let keys = h.deployment.current().fixture.keys.clone();
    let v3 = deploy::verify_embedded_pinned(
        f3,
        &h.pool,
        &cfg("ordo_vserve_unfit"),
        keys,
        ordo_conformance::fixture_space(),
        "v3",
    )
    .await
    .expect("v3 gates in its shadow");
    h.deployment.install(v3);
    h.deployment.note_pin(TWIN, "v3", 1, 0);
    let (st, body) = call(&h, Method::GET, "std.memos.list", TWIN, TWIN_OWNER, None).await;
    assert_eq!(st, 503, "{body}");
    assert!(
        !body.to_string().contains("tags"),
        "plain to the caller: {body}"
    );
    let why = h
        .spaces
        .quarantined()
        .into_iter()
        .find(|(s, _)| s == TWIN)
        .map(|(_, w)| w)
        .expect("held, with a reason");
    assert!(why.contains("`v3`") && why.contains("tags"), "{why}");
    assert!(
        why.contains("ordo versions pin"),
        "the reason names the fix: {why}"
    );
}

// ── the pin stream, over a real registry ─────────────────────────────────

const V010: &str = "domain tasks @ 0.1.0

entity task {
  title: text
  done:  bool
}

op openTasks unbounded =
  task[title, done]
";

const V020: &str = "domain tasks @ 0.2.0

entity task {
  title: text
  done:  bool
  due:   int?
}

op openTasks unbounded =
  task[title, done]
";

const V030: &str = "domain tasks @ 0.3.0

entity task {
  title: text
  done:  bool
  due:   int?
  note:  text?
}

op openTasks unbounded =
  task[title, done]
";

async fn registry_db(stem: &str) -> (Pool, PoolConfig) {
    let pg = Pg::scoped(stem);
    let c = PoolConfig {
        db: pg.db.clone(),
        size: 4,
        ..PoolConfig::from_env()
    };
    let pool = Pool::connect(c.clone()).await.expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    (pool, c)
}

async fn apply(pool: &Pool, source: &str) {
    let program = Program::compile(source).expect("compiles");
    apply_op::run_program(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "dev:ada",
        "t",
        true,
    )
    .await
    .expect("applies");
}

async fn tenant(pool: &Pool, id: &str) {
    assert!(tenants::reserve(pool, id, id, "dev:ada", 10, None)
        .await
        .expect("reserved"));
    tenants::activate(pool, id, 11).await.expect("active");
}

/// A node over the registry, booted on the active version.
async fn node(
    pool: &Pool,
    c: &PoolConfig,
) -> (Arc<Deployment>, Arc<ordo_host::pin_stream::PinStream>) {
    let keys = Fixture::from_source("", "").expect("empty").keys.clone();
    let space = ordo_conformance::fixture_space();
    let active = deploy::load_active(pool, c, keys.clone(), space.clone())
        .await
        .expect("loads");
    let d = Deployment::boot(active);
    let s = ordo_host::pin_stream::PinStream::new(d.clone(), pool.clone(), c.clone(), keys, space);
    (d, s)
}

/// **The pin stream** (ask 17): a pin moved on one tenant reaches the node
/// by the stream, the node loads that version beside the active one, and
/// drops it once an activation moves every pin off it.
#[tokio::test(flavor = "multi_thread")]
async fn a_pin_reaches_a_node_by_the_stream_and_loads_its_version() {
    let (pool, c) = registry_db("ordo_vserve_stream").await;
    tenant(&pool, "spc_a").await;
    tenant(&pool, "spc_b").await;
    apply(&pool, V010).await;
    apply(&pool, V020).await;
    let (d, stream) = node(&pool, &c).await;
    let boot = stream.reload().await.expect("whole read");
    assert!(
        boot.loaded.is_empty(),
        "every tenant is on the active version: {boot:?}"
    );
    assert_eq!(d.pin_of("spc_a").map(|p| p.0), Some("0.2.0".into()));

    let moved = Versions::on(&pool)
        .pin("spc_a", "#1", "deployment-owner:ada", 50)
        .await
        .expect("pinned");
    assert_eq!(
        (moved.from.as_deref(), moved.to.as_str()),
        (Some("0.2.0"), "0.1.0")
    );
    let read = stream.tail().await.expect("tail");
    assert_eq!(read.moved, vec!["spc_a".to_string()]);
    assert_eq!(read.loaded, vec!["0.1.0".to_string()], "{read:?}");
    let (loaded, epoch) = d.for_space("spc_a").expect("served");
    assert_eq!(loaded.version, "0.1.0");
    assert_eq!(Some(epoch.unwrap()), Some(moved.epoch));
    assert_eq!(d.for_space("spc_b").expect("served").0.version, "0.2.0");
    let last = Versions::on(&pool)
        .pin_history("spc_a")
        .await
        .expect("history");
    assert_eq!(last.last().map(|c| c.cause.as_str()), Some("pin"));
    assert!(
        stream.tail().await.expect("again").moved.is_empty(),
        "read once"
    );

    // An activation moves every pin. Until this node swaps to it, spc_a is
    // served 0.1.0, the version it held before; once it has, the node
    // drops the version no pin names.
    apply(&pool, V030).await;
    let read = stream.reload().await.expect("whole read");
    assert!(read.dropped.is_empty(), "{read:?}");
    assert_eq!(d.for_space("spc_a").expect("served").0.version, "0.1.0");
    let keys = Fixture::from_source("", "").expect("empty").keys.clone();
    let v3 = deploy::load_active(&pool, &c, keys, ordo_conformance::fixture_space())
        .await
        .expect("loads");
    assert!(matches!(d.swap(v3), Swapped::Installed { .. }));
    assert!(d.loaded("0.1.0").is_none(), "dropped once the swap lands");
    assert!(d.loaded("0.2.0").is_none(), "{:?}", d.served_versions());
    assert_eq!(d.for_space("spc_a").expect("served").0.version, "0.3.0");
}

/// The move's refusals: a revoked version, and a tenant there is none of.
#[tokio::test(flavor = "multi_thread")]
async fn a_pin_to_a_revoked_version_or_no_tenant_is_refused() {
    let (pool, _) = registry_db("ordo_vserve_refuse").await;
    tenant(&pool, "spc_a").await;
    apply(&pool, V010).await;
    apply(&pool, V020).await;
    let v = Versions::on(&pool);
    v.revoke("0.1.0", "a flaw", None, "deployment-owner:ada", 40)
        .await
        .expect("revoked");
    let e = v
        .pin("spc_a", "0.1.0", "deployment-owner:ada", 50)
        .await
        .expect_err("refused");
    assert!(matches!(e, VersionRefusal::Revoked { .. }), "{e}");
    assert!(e.to_string().contains("ordo versions unrevoke"), "{e}");
    let e = v
        .pin("spc_none", "0.2.0", "deployment-owner:ada", 50)
        .await
        .expect_err("refused");
    assert!(matches!(e, VersionRefusal::NoTenant { .. }), "{e}");
    assert!(e.to_string().contains("ordo tenants list"), "{e}");
}

/// **Which moves wait for a maintenance window** (audit 2026-11-05): none
/// yet. A window set for every tenant, in the future, holds nothing back:
/// an activation and a pin move the tenant at once. A tenant left on a
/// version revoked while it was not active stays there; moving it in its
/// window or on demand is ruled (2026-11-05) and lands in session 2.
#[tokio::test(flavor = "multi_thread")]
async fn no_move_waits_for_a_window_yet_and_a_revoked_versions_tenant_stays() {
    let (pool, _) = registry_db("ordo_vserve_windows").await;
    tenant(&pool, "spc_a").await;
    tenant(&pool, "spc_b").await;
    apply(&pool, V010).await;
    ordo_host::windows::Windows::on(&pool)
        .set(
            &ordo_host::windows::WindowSpec {
                id: None,
                target: ordo_host::windows::Target::All,
                schedule: ordo_host::windows::Schedule::Range {
                    start: 4_000_000_000,
                    end: 4_000_003_600,
                },
                message: "later".into(),
            },
            "deployment-owner:ada",
            1_000,
        )
        .await
        .expect("a window, far ahead");
    apply(&pool, V020).await;
    let v = Versions::on(&pool);
    let on = |p: Option<ordo_host::versions::Pin>| p.and_then(|p| p.version);
    assert_eq!(
        on(v.pin_of("spc_a").await.unwrap()),
        Some("0.2.0".into()),
        "an activation moves at once"
    );
    v.pin("spc_b", "0.1.0", "deployment-owner:ada", 60)
        .await
        .expect("pinned");
    assert_eq!(
        on(v.pin_of("spc_b").await.unwrap()),
        Some("0.1.0".into()),
        "a pin moves at once"
    );
    v.revoke("0.1.0", "a flaw", None, "deployment-owner:ada", 70)
        .await
        .expect("revoked");
    assert_eq!(
        on(v.pin_of("spc_b").await.unwrap()),
        Some("0.1.0".into()),
        "a tenant on a revoked version stays until its move lands (session 2)"
    );
}
