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

op addNote($title: text) =
  set cards { title: $title }

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

op addNote($title: text) =
  set cards { title: $title, watched: true }

op onlyTwo =
  cards[title]
  sort title
  take 5

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
    store(pool, source, true).await;
}

/// Store `source`, and make it the active version when `activate`.
async fn store(pool: &Pool, source: &str, activate: bool) {
    let program = Program::compile(source).expect("compiles");
    run_all_at_once(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "dev:ada",
        "t",
        activate,
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

/// At most 3 nodes at once in this process: each holds pools and compiles
/// two versions, and eight at once ran the local server out of
/// connections. nextest's `serial-in-file` group does the same on CI.
static AT_ONCE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);

struct Node {
    _turn: tokio::sync::SemaphorePermit<'static>,
    app: axum::Router,
    pool: Pool,
    deployment: std::sync::Arc<ordo_host::deploy::Deployment>,
    access: std::sync::Arc<ordo_host::runtime::SwappableAccess>,
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
    boot_with(stem, Some("0")).await
}

/// [`boot`], with `versions.min_residency` as given (`None`: the default).
async fn boot_with(stem: &str, residency: Option<&str>) -> Node {
    let turn = AT_ONCE.acquire().await.expect("open");
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
    if let Some(r) = residency {
        Versions::on(&pool)
            .set_setting(
                ordo_host::versions::VersionSetting::MinResidency,
                Some(r),
                "dev:ada",
                "the test's residency",
                12,
            )
            .await
            .expect("min_residency");
    }
    let rt = runtime::assemble(config(&pg.db)).await.expect("assembles");
    for space in [SPC_A, SPC_B] {
        let _ = rt.keys.create_space(&ordo_crypto::SpaceId(space.into()));
    }
    let app = runtime::door(&rt);
    let deployment = rt.deployment.clone();
    let access = rt.access.clone();
    let serving = tokio::spawn(async move {
        let _ = runtime::serve(rt).await;
    });
    let node = Node {
        _turn: turn,
        app,
        pool,
        deployment,
        access,
        serving,
    };
    wait(|| node.deployment.loaded("0.1.0").is_some()).await;
    node
}

/// Up to 40 seconds: eight nodes booting at once each compile two
/// versions.
async fn wait(mut done: impl FnMut() -> bool) {
    for _ in 0..800 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never happened");
}

struct Answer {
    status: u16,
    headers: axum::http::HeaderMap,
    body: Value,
}

impl Answer {
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    }

    fn code(&self) -> String {
        self.body["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

async fn call(
    node: &Node,
    method: &str,
    uri: &str,
    space: &str,
    who: &str,
    body: Option<Value>,
) -> Answer {
    call_keyed(node, method, uri, space, who, body, None).await
}

async fn call_keyed(
    node: &Node,
    method: &str,
    uri: &str,
    space: &str,
    who: &str,
    body: Option<Value>,
    key: Option<&str>,
) -> Answer {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-ordo-space", space)
        .header("authorization", format!("Bearer {}", mint(who)));
    if let Some(k) = key {
        req = req.header("idempotency-key", k);
    }
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
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    Answer {
        status,
        headers,
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

// ── the client contract (§1.9) ───────────────────────────────────────────

/// **Every answer carries the tenant's version** (§1.9), as
/// `x-ordo-tenant-version`: `spc_a`'s are 0.1.0's, `spc_b`'s 0.2.0's.
/// **An op the tenant's version lacks gets its own refusal**: `onlyTwo`,
/// which 0.2.0 alone declares, answers `op_not_in_version` in `spc_a`,
/// distinct from the unknown op's `access_denied`. A caller who may not call
/// it there gets the unknown op's answer, so the code maps nothing for a
/// stranger.
#[tokio::test(flavor = "multi_thread")]
async fn every_answer_names_the_tenants_version_and_a_missing_op_says_so() {
    let node = boot("ordo_vbg_contract").await;
    let a = call(&node, "GET", "/op/cardList", SPC_A, "ada", None).await;
    assert_eq!(a.status, 200, "{}", a.body);
    assert_eq!(a.header("x-ordo-tenant-version").as_deref(), Some("0.1.0"));
    let b = call(&node, "GET", "/op/cardList", SPC_B, "bo", None).await;
    assert_eq!(b.header("x-ordo-tenant-version").as_deref(), Some("0.2.0"));

    let only = call(&node, "GET", "/op/onlyTwo", SPC_B, "bo", None).await;
    assert_eq!(only.status, 200, "{}", only.body);
    let missing = call(&node, "GET", "/op/onlyTwo", SPC_A, "ada", None).await;
    assert_eq!(missing.code(), "op_not_in_version", "{}", missing.body);
    assert_eq!(missing.status, 409, "{}", missing.body);
    let text = missing.body.to_string();
    assert!(!text.contains("0.1.0") && !text.contains("0.2.0"), "{text}");

    let unknown = call(&node, "GET", "/op/noSuchOp", SPC_A, "ada", None).await;
    assert_eq!(unknown.code(), "access_denied", "{}", unknown.body);
    // `bo` is no member of `spc_a`: the unknown op's answer, not the new one.
    let stranger = call(&node, "GET", "/op/onlyTwo", SPC_A, "bo", None).await;
    assert_eq!(stranger.code(), unknown.code(), "{}", stranger.body);
    assert_eq!(stranger.status, unknown.status);
}

/// **An idempotency key is honoured across one switch** (§1.9). `addNote`
/// differs between 0.1.0 and 0.2.0, so its version hash moves with the
/// pin. A write sent under 0.1.0 and retried with its key after the move
/// replays the first answer and writes nothing more. A different payload
/// under the key is still `idempotency_mismatch`.
#[tokio::test(flavor = "multi_thread")]
async fn an_idempotency_key_is_honoured_across_one_switch() {
    let node = boot("ordo_vbg_idem").await;
    let body = || Some(json!({ "title": "once" }));
    let first = call_keyed(
        &node,
        "POST",
        "/op/addNote",
        SPC_A,
        "ada",
        body(),
        Some("k-1"),
    )
    .await;
    assert_eq!(first.status, 200, "{}", first.body);
    let hash_before = first.header("x-ordo-op-version");

    Versions::on(&node.pool)
        .pin(SPC_A, "0.2.0", "deployment-owner:ada", 60)
        .await
        .expect("moved");
    wait(|| node.deployment.loaded("0.1.0").is_none()).await;

    let again = call_keyed(
        &node,
        "POST",
        "/op/addNote",
        SPC_A,
        "ada",
        body(),
        Some("k-1"),
    )
    .await;
    assert_eq!(again.status, 200, "{}", again.body);
    assert_ne!(
        again.header("x-ordo-op-version"),
        hash_before,
        "the op changed with the version"
    );
    assert_eq!(
        again.header("x-ordo-idempotent-replay").as_deref(),
        Some("true"),
        "the first answer replayed: {}",
        again.body
    );
    let titles: Vec<String> = cards(&node, SPC_A, "ada")
        .await
        .into_iter()
        .map(|c| c.0)
        .collect();
    assert_eq!(titles, vec!["once".to_string()], "written once");

    let other = call_keyed(
        &node,
        "POST",
        "/op/addNote",
        SPC_A,
        "ada",
        Some(json!({ "title": "else" })),
        Some("k-1"),
    )
    .await;
    assert_eq!(other.code(), "idempotency_mismatch", "{}", other.body);
}

/// **The tenant admin sees its version's label** (§1.9):
/// `std.space.version` answers the tenant's version, order number, label
/// and whether it is revoked. A member below manager is refused.
#[tokio::test(flavor = "multi_thread")]
async fn the_tenant_admin_reads_its_versions_label() {
    let node = boot("ordo_vbg_label").await;
    Versions::on(&node.pool)
        .set_label("0.1.0", Some("spring release"))
        .await
        .expect("labelled");
    let a = call(&node, "GET", "/op/std.space.version", SPC_A, "ada", None).await;
    assert_eq!(a.status, 200, "{}", a.body);
    let row = &a.body["rows"][0];
    assert_eq!(row["version"], "0.1.0", "{row}");
    assert_eq!(row["ordinal"], 1, "{row}");
    assert_eq!(row["label"], "spring release", "{row}");
    assert_eq!(row["revoked"], false, "{row}");
    // The refusal: no member of the tenant.
    let b = call(&node, "GET", "/op/std.space.version", SPC_A, "bo", None).await;
    assert_eq!(b.code(), "access_denied", "{}", b.body);
}

// ── the load guards (§1.4 step 5, §1.5, §7) ──────────────────────────────

async fn set_guard(node: &Node, setting: ordo_host::versions::VersionSetting, value: &str) {
    Versions::on(&node.pool)
        .set_setting(setting, Some(value), "dev:ada", "the test", 80)
        .await
        .expect("set");
}

/// Ask every node to read the pins now, as a pin change would.
async fn nudge(node: &Node) {
    node.pool
        .rows(&format!(
            "SELECT pg_notify('{}', '')",
            ordo_host::versions::PINS_CHANNEL
        ))
        .await
        .expect("notify");
}

/// **`versions.min_residency`**: at its default, 10 minutes, a version no
/// pin names any more stays loaded, so a pin moved back does not compile
/// it again. Set to `0`, it drops at the next read. A value that is not a
/// duration is refused, naming the setting and the command.
#[tokio::test(flavor = "multi_thread")]
async fn a_version_stays_loaded_for_min_residency() {
    use ordo_host::versions::VersionSetting;
    let node = boot_with("ordo_vbg_residency", None).await;
    Versions::on(&node.pool)
        .pin(SPC_A, "0.2.0", "deployment-owner:ada", 60)
        .await
        .expect("moved");
    // The move is read, and 0.1.0 is still served though nothing names it.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        node.deployment.loaded("0.1.0").is_some(),
        "kept for versions.min_residency"
    );

    let refused = Versions::on(&node.pool)
        .set_setting(
            VersionSetting::MinResidency,
            Some("soon"),
            "dev:ada",
            "t",
            81,
        )
        .await
        .expect_err("not a duration");
    let text = refused.to_string();
    assert!(
        text.contains("versions.min_residency") && text.contains("ordo versions set min-residency"),
        "{text}"
    );

    set_guard(&node, VersionSetting::MinResidency, "0").await;
    wait(|| node.deployment.loaded("0.1.0").is_none()).await;
}

/// **The version a node booted on keeps `versions.min_residency` too**
/// (Reported fix, 2026-11-24). It is installed by no swap, and its load
/// clock was left at 0, so it read as loaded since 1970: the moment no pin
/// named it, it dropped, and a pin moved back had it compiled again while
/// its tenants answered `space_unavailable`. Here the node boots on 0.2.0,
/// 0.1.0 is activated again and every pin leaves 0.2.0: at the default
/// residency, 0.2.0 stays loaded.
#[tokio::test(flavor = "multi_thread")]
async fn the_version_a_node_booted_on_stays_loaded_for_min_residency() {
    let node = boot_with("ordo_vbg_boot_residency", None).await;
    assert_eq!(
        node.deployment.current().version,
        "0.2.0",
        "booted on 0.2.0"
    );
    Versions::on(&node.pool)
        .activate(
            "0.1.0",
            "deployment-owner:ada",
            ordo_commandlog::Timestamp(70),
        )
        .await
        .expect("0.1.0 active again");
    wait(|| node.deployment.current().version == "0.1.0").await;
    // Every pin is on 0.1.0 now; reads of the pins have run since.
    for _ in 0..3 {
        nudge(&node).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(
        node.deployment.loaded("0.2.0").is_some(),
        "the boot version is kept for versions.min_residency"
    );
}

/// **`versions.idle_evict_after`**: 0.1.0 serves nothing for a second and
/// is unloaded. Nothing loads it again by itself. The next signed-in
/// request for its tenant wakes it, waits for the load and is answered as
/// ever, by 0.1.0. Past `WAKE_WAIT` it would answer `server_busy` with
/// `Retry-After`, which the SDK retries.
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_version_is_unloaded_and_a_signed_in_request_loads_it_again() {
    use ordo_host::versions::VersionSetting;
    let node = boot("ordo_vbg_idle").await;
    set_guard(&node, VersionSetting::IdleEvictAfter, "1s").await;
    // `tickOne` runs in spc_a every 2 seconds, and a run is traffic. A
    // read of the pins that lands in the quiet second between two runs
    // evicts it; after that nothing of 0.1.0 is wired, so no run comes.
    let evicted = eventually(30, || async {
        nudge(&node).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        node.deployment.loaded("0.1.0").is_none()
    })
    .await;
    assert!(evicted, "0.1.0 was never idle for a second");
    assert_eq!(node.deployment.evicted(), vec!["0.1.0".to_string()]);

    // Not loaded again by a read with no request behind it.
    nudge(&node).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(node.deployment.loaded("0.1.0").is_none(), "stays unloaded");

    // Off from here, so the reload is not evicted again under the test.
    set_guard(&node, VersionSetting::IdleEvictAfter, "off").await;
    // The one request that wakes it waits for the load and answers as
    // ever: standard behaviour, no error for the person.
    let first = call(&node, "GET", "/op/cardList", SPC_A, "ada", None).await;
    assert_eq!(first.status, 200, "{}", first.body);
    assert_eq!(
        first.header("x-ordo-tenant-version").as_deref(),
        Some("0.1.0")
    );
    assert!(node.deployment.loaded("0.1.0").is_some());
}

/// **Per-node health shows a version still loading apart from one
/// loaded**: `$nodes.loading` round-trips, and `/health/deploy` carries it
/// beside `alsoServing`.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_says_which_versions_it_is_still_loading() {
    let node = boot("ordo_vbg_loading").await;
    let row = ordo_exec::nodes::Node {
        node: "other-node".into(),
        serving: "0.2.0".into(),
        also_serving: vec!["0.1.0".into()],
        loading: vec!["0.3.0".into()],
        beat_secs: 5,
        ..Default::default()
    };
    ordo_exec::nodes::beat(&node.pool, &row)
        .await
        .expect("beat");
    let live = ordo_exec::nodes::live(&node.pool).await.expect("live");
    let other = live.iter().find(|n| n.node == "other-node").expect("row");
    assert_eq!(other.loading, vec!["0.3.0".to_string()]);
    assert_eq!(other.also_serving, vec!["0.1.0".to_string()]);
    // This node: once 0.1.0 lands it is served, and no longer loading.
    let landed = eventually(10, || async {
        let live = ordo_exec::nodes::live(&node.pool).await.expect("live");
        live.iter().any(|n| {
            n.node == "versions-background-test"
                && n.loading.is_empty()
                && n.also_serving == vec!["0.1.0".to_string()]
        })
    })
    .await;
    assert!(landed, "{:?}", ordo_exec::nodes::live(&node.pool).await);
}

// ── a version's new ops while the door rebuilds ──────────────────────────

/// 0.3.0: 0.2.0 and `onlyThree`, an op no earlier version declares.
fn v030() -> String {
    V020.replace("domain board @ 0.2.0", "domain board @ 0.3.0")
        + "\nop onlyThree =\n  cards[title]\n  sort title\n  take 5\n"
}

/// `spc_a` served 0.3.0 by this node.
fn on_v030(node: &Node) -> bool {
    node.deployment
        .serving(SPC_A)
        .is_some_and(|l| l.version == "0.3.0")
}

/// **A version's new op is never denied while the door rebuilds**
/// (reported fix new-ops-denied-during-rebuild). `spc_a` moves to 0.3.0,
/// which adds `onlyThree`. Every rebuild is held, so the door serving is
/// the one built before 0.3.0 loaded: `ada`, the tenant's owner, is told
/// `server_busy` with a `Retry-After`, never `access_denied`. Once the
/// rebuild installs she is answered, and `bo`, no member, is denied.
///
/// A regression looks like `access_denied` for `ada` while the rebuild is
/// held.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_versions_op_is_busy_not_denied_until_the_door_rebuilds() {
    let node = boot("ordo_vbg_newop").await;
    store(&node.pool, &v030(), false).await;
    let held = node.access.hold_rebuilds().await;
    Versions::on(&node.pool)
        .pin(SPC_A, "0.3.0", "deployment-owner:ada", 60)
        .await
        .expect("moved");
    wait(|| on_v030(&node)).await;

    let busy = call(&node, "GET", "/op/onlyThree", SPC_A, "ada", None).await;
    assert_eq!(busy.code(), "server_busy", "{}", busy.body);
    assert_eq!(busy.status, 503, "{}", busy.body);
    assert!(busy.header("retry-after").is_some(), "{:?}", busy.headers);

    drop(held);
    let answered = eventually(30, || async {
        call(&node, "GET", "/op/onlyThree", SPC_A, "ada", None)
            .await
            .status
            == 200
    })
    .await;
    assert!(answered, "the rebuild installed and ada was answered");
    let stranger = call(&node, "GET", "/op/onlyThree", SPC_A, "bo", None).await;
    assert_eq!(stranger.code(), "access_denied", "{}", stranger.body);
}

/// The same move with nothing held: the first call after 0.3.0 loads, and
/// every call after it until one is answered, is answered or busy, and
/// never denied.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_versions_op_called_the_moment_it_loads_is_never_denied() {
    let node = boot("ordo_vbg_newop_now").await;
    store(&node.pool, &v030(), false).await;
    Versions::on(&node.pool)
        .pin(SPC_A, "0.3.0", "deployment-owner:ada", 60)
        .await
        .expect("moved");
    wait(|| on_v030(&node)).await;
    let until = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let a = call(&node, "GET", "/op/onlyThree", SPC_A, "ada", None).await;
        if a.status == 200 {
            break;
        }
        assert_eq!(a.code(), "server_busy", "{}", a.body);
        assert!(std::time::Instant::now() < until, "never answered");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `apply_op::run_program` with `rollout.strategy` set to `all` first. With
/// it unset, a deployment that is not dev rolls an apply out a batch at a
/// time (ruled 2026-11-25); these tests apply and expect every tenant on
/// the new version at once.
async fn run_all_at_once(
    pool: &ordo_exec::Pool,
    pools: &dyn ordo_exec::route::PoolResolver,
    program: &ordo_host::schema_apply::Program,
    notes: Vec<String>,
    actor: &str,
    op: &str,
    activate: bool,
) -> Result<ordo_host::apply_op::Report, ordo_host::apply_op::ApplyRefused> {
    ordo_host::rollouts::Rollouts::on(pool)
        .set_setting(
            "rollout.strategy",
            Some("all"),
            "test",
            "every tenant at once",
            0,
        )
        .await
        .expect("rollout.strategy all");
    ordo_host::apply_op::run_program(pool, pools, program, notes, actor, op, activate).await
}
