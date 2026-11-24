//! Increment 3 of `plan-versions.md`, the remainder: background work and
//! the client contract under pins, tried on a running server
//! (`runtime::assemble`, then `runtime::serve`, with requests through
//! `runtime::door`).
//!
//! - **Schedules and listeners are the union over served versions**
//!   (§1.10). A schedule only 0.1.0 declares fires in the tenant pinned to
//!   0.1.0 and never in the tenant on 0.2.0, which records no skip for it.
//! - **The cursor is handed over at a switch** (§1.10). A routine both
//!   versions declare keeps its cursor when the tenant moves, so no change
//!   is run twice and none is missed. A listener only the old version
//!   declares has its cursor retired.
//!
//! A regression looks like a pinned tenant's routine never running, a
//! routine running in a tenant whose version lacks it, a change run twice
//! after a move, or a dead cursor holding the log.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use ordo_conformance::{Fixture, Pg};
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::runtime::{self, CustodyChoice, RuntimeConfig};
use ordo_host::schema_apply::Program;
use ordo_host::tenants;
use ordo_host::versions::Versions;
use serde_json::{json, Value};
use tower::util::ServiceExt;

const SECRET: [u8; 32] = [13u8; 32];
const SPC_A: &str = "spc_a";
const SPC_B: &str = "spc_b";

/// 0.1.0: `follow` marks a new card `v1`; `watchOne` marks it watched; and
/// `tickOne` adds a tick every 2 seconds. 0.2.0 declares neither of the
/// last two.
const V010: &str = "domain board @ 0.1.0

entity cards {
  title:   text
  seen:    text = \"\"
  watched: bool = false
}

entity ticks {
  n: int
}

op addCard($title: text) =
  set cards { title: $title }

op cardList =
  cards[title, seen, watched]
  sort title
  take 50

op tickList =
  ticks[n]
  take 500

routine follow {
  trigger when board.cards.created
  run:
    set cards(seen == \"\") { seen: \"v1\" }
}

routine watchOne {
  trigger when board.cards.created
  run:
    set cards(watched == false) { watched: true }
}

routine tickOne {
  trigger every 2s
  run:
    set ticks { n: 1 }
}
";

/// 0.2.0: `follow` marks a new card `v2`. No `watchOne`, no `tickOne`.
const V020: &str = "domain board @ 0.2.0

entity cards {
  title:   text
  seen:    text = \"\"
  watched: bool = false
}

entity ticks {
  n: int
}

op addCard($title: text) =
  set cards { title: $title }

op cardList =
  cards[title, seen, watched]
  sort title
  take 50

op tickList =
  ticks[n]
  take 500

routine follow {
  trigger when board.cards.created
  run:
    set cards(seen == \"\") { seen: \"v2\" }
}
";

fn pg_cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 8,
        ..PoolConfig::from_env()
    }
}

async fn apply(pool: &Pool, source: &str) {
    let program = Program::compile(source).expect("compiles");
    ordo_host::apply_op::run_program(
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

fn free_addr() -> std::net::SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn config(db: &str) -> RuntimeConfig {
    RuntimeConfig {
        blob_routing: ordo_host::runtime::BlobRouting::Single,
        pg: pg_cfg(db),
        addr: free_addr(),
        shutdown_grace: Duration::from_secs(1),
        space: ordo_crypto::SpaceId("system".into()),
        tightened: Vec::new(),
        custody: CustodyChoice::Test,
        token_secret: SECRET,
        page_key: ordo_api::PageKey::from_secret(&[14u8; 32]),
        checkpoint: None,
        security_checkpoint: false,
        apex_domain: None,
        cors: None,
        blob: None,
        exit_bundles: None,
        import_bundles: None,
        replica: "versions-background-test".into(),
        usage_flush: Duration::from_secs(60),
        usage_rollup: Duration::from_secs(3600),
        sync: Default::default(),
        auth: None,
        credentials: None,
        presence: Default::default(),
        pausing: Default::default(),
        version_moves: Default::default(),
        delegable: Default::default(),
        access_template: Default::default(),
        deployment_owners: Vec::new(),
        provisioning: Default::default(),
        recovery: Default::default(),
        second_factor: Default::default(),
        mail: None,
    }
}

fn mint(principal: &str) -> String {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    ordo_identity::mint(
        &ordo_identity::TokenKey::from_secret(&SECRET),
        &ordo_identity::TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(u64::MAX / 2),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

struct Node {
    app: axum::Router,
    pool: Pool,
    deployment: std::sync::Arc<ordo_host::deploy::Deployment>,
    serving: tokio::task::JoinHandle<()>,
}

impl Drop for Node {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

/// 0.1.0 then 0.2.0 applied, 0.2.0 active, `spc_a` pinned to 0.1.0 before
/// the node starts, and `spc_b` on 0.2.0.
async fn boot(stem: &str) -> Node {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(pg_cfg(&pg.db)).await.expect("pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    for id in [SPC_A, SPC_B] {
        assert!(tenants::reserve(&pool, id, id, "dev:ada", 10, None)
            .await
            .expect("reserved"));
        tenants::activate(&pool, id, 11).await.expect("active");
    }
    apply(&pool, V010).await;
    apply(&pool, V020).await;
    let keys = Fixture::from_source("", "").expect("empty").keys.clone();
    for (space, who) in [(SPC_A, "ada"), (SPC_B, "bo")] {
        ordo_host::provision::provision_space(
            &pool,
            keys.as_ref(),
            space,
            &ordo_host::provision::FoundingOwner {
                member_id: who,
                principal: who,
                display_name: who,
            },
        )
        .await
        .expect("provisions");
    }
    Versions::on(&pool)
        .pin(SPC_A, "0.1.0", "deployment-owner:ada", 50)
        .await
        .expect("pinned");
    let rt = runtime::assemble(config(&pg.db)).await.expect("assembles");
    for space in [SPC_A, SPC_B] {
        let _ = rt.keys.create_space(&ordo_crypto::SpaceId(space.into()));
    }
    let app = runtime::door(&rt);
    let deployment = rt.deployment.clone();
    let serving = tokio::spawn(async move {
        let _ = runtime::serve(rt).await;
    });
    let node = Node {
        app,
        pool,
        deployment,
        serving,
    };
    wait(|| node.deployment.loaded("0.1.0").is_some()).await;
    node
}

async fn wait(mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never happened");
}

struct Answer {
    status: u16,
    body: Value,
}

async fn call(
    node: &Node,
    method: &str,
    uri: &str,
    space: &str,
    who: &str,
    body: Option<Value>,
) -> Answer {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-ordo-space", space)
        .header("authorization", format!("Bearer {}", mint(who)));
    let req = match body {
        Some(v) => req
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => {
            req = req.header("accept", "application/json");
            req.body(Body::empty()).unwrap()
        }
    };
    let resp = node.app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    Answer {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

async fn add(node: &Node, space: &str, who: &str, title: &str) {
    let a = call(
        node,
        "POST",
        "/op/addCard",
        space,
        who,
        Some(json!({ "title": title })),
    )
    .await;
    assert_eq!(a.status, 200, "{}", a.body);
}

/// Each card's title, `seen` and `watched`, by title.
async fn cards(node: &Node, space: &str, who: &str) -> Vec<(String, String, bool)> {
    let a = call(node, "GET", "/op/cardList", space, who, None).await;
    assert_eq!(a.status, 200, "{}", a.body);
    a.body["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|r| {
            (
                r["title"].as_str().unwrap_or_default().to_string(),
                r["seen"].as_str().unwrap_or_default().to_string(),
                r["watched"].as_bool().unwrap_or(false),
            )
        })
        .collect()
}

async fn ticks(node: &Node, space: &str, who: &str) -> usize {
    let a = call(node, "GET", "/op/tickList", space, who, None).await;
    assert_eq!(a.status, 200, "{}", a.body);
    a.body["rows"].as_array().map_or(0, |r| r.len())
}

async fn eventually<F, Fut>(secs: u64, mut done: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let until = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < until {
        if done().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

/// **The union, and each tenant's own** (§1.10). `tickOne` is declared by
/// 0.1.0 alone: it fires in `spc_a` and never in `spc_b`, and `spc_b`
/// records no skip for it. A new card runs each tenant's own `follow`.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_only_the_pinned_version_declares_runs_only_in_its_tenants() {
    let node = boot("ordo_vbg_union").await;

    let ran = eventually(20, || async { ticks(&node, SPC_A, "ada").await > 0 }).await;
    assert!(ran, "0.1.0's schedule never ran in the tenant pinned to it");
    // The negative: two more periods, and nothing in the 0.2.0 tenant.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(ticks(&node, SPC_B, "bo").await, 0, "0.2.0 has no tickOne");
    let skips = ordo_host::routines::skips(&node.pool, SPC_B).await.unwrap();
    assert!(
        skips.iter().all(|s| s.routine != "routine:tickOne"),
        "no run was even enqueued for spc_b: {skips:?}"
    );

    add(&node, SPC_A, "ada", "a1").await;
    add(&node, SPC_B, "bo", "b1").await;
    let both = eventually(20, || async {
        cards(&node, SPC_A, "ada").await == vec![("a1".into(), "v1".into(), true)]
            && cards(&node, SPC_B, "bo").await == vec![("b1".into(), "v2".into(), false)]
    })
    .await;
    assert!(
        both,
        "each tenant ran its own version's routines: a {:?}, b {:?}",
        cards(&node, SPC_A, "ada").await,
        cards(&node, SPC_B, "bo").await
    );
}

/// **The cursor is handed over at a switch** (§1.10). `spc_a` moves from
/// 0.1.0 to 0.2.0. `follow`, declared by both, keeps its cursor: the card
/// added before the move is not run again, the one after it runs once
/// under 0.2.0. `watchOne`'s cursor, which 0.2.0 does not declare, is
/// retired, and `tickOne` stops.
#[tokio::test(flavor = "multi_thread")]
async fn a_pin_switch_hands_the_cursor_over_and_retires_what_the_new_version_lacks() {
    let node = boot("ordo_vbg_handover").await;
    let id = ordo_commandlog::SpaceId(SPC_A.into());
    // The drain registers a tenant's cursors at the head on first reach; a
    // card added before that is history no routine reacts to.
    let registered = eventually(20, || async {
        let held = ordo_exec::prune::consumers(&node.pool, &id).await.unwrap();
        held.iter().any(|c| c.name.starts_with("watchOne:"))
    })
    .await;
    assert!(registered, "spc_a's cursors were never registered");
    add(&node, SPC_A, "ada", "a1").await;
    let first = eventually(20, || async {
        cards(&node, SPC_A, "ada").await == vec![("a1".into(), "v1".into(), true)]
    })
    .await;
    assert!(first, "{:?}", cards(&node, SPC_A, "ada").await);
    let before = ordo_exec::prune::consumers(&node.pool, &id).await.unwrap();
    let follow_at = before
        .iter()
        .find(|c| c.name.starts_with("follow:"))
        .map(|c| c.position)
        .expect("follow's cursor");
    assert!(before.iter().any(|c| c.name.starts_with("watchOne:")));

    Versions::on(&node.pool)
        .pin(SPC_A, "0.2.0", "deployment-owner:ada", 60)
        .await
        .expect("moved");
    wait(|| node.deployment.loaded("0.1.0").is_none()).await;

    add(&node, SPC_A, "ada", "a2").await;
    let after = eventually(20, || async {
        cards(&node, SPC_A, "ada").await
            == vec![
                ("a1".into(), "v1".into(), true),
                ("a2".into(), "v2".into(), false),
            ]
    })
    .await;
    assert!(after, "{:?}", cards(&node, SPC_A, "ada").await);

    let held = ordo_exec::prune::consumers(&node.pool, &id).await.unwrap();
    let follow = held
        .iter()
        .find(|c| c.name.starts_with("follow:"))
        .expect("follow kept its cursor");
    assert!(follow.position > follow_at, "it read on from where it was");
    assert!(
        !held.iter().any(|c| c.name.starts_with("watchOne:")),
        "the cursor of a routine 0.2.0 lacks is retired: {:?}",
        held.iter().map(|c| c.name.clone()).collect::<Vec<_>>()
    );
    let n = ticks(&node, SPC_A, "ada").await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(ticks(&node, SPC_A, "ada").await, n, "tickOne stopped");
}
