//! Increment 3 of `plan-versions.md`, session 3: what each loaded version
//! carries with it, tried on a running server (`runtime::assemble`, then
//! `runtime::serve`, with requests through `runtime::door`).
//!
//! - **Digest at load** (§1.4 step 4, §6 conflict 4): a stored version
//!   whose source fingerprint no longer matches is refused by name, and
//!   its tenants are closed rather than served something else.
//! - **One function table per loaded version** (§1.10): a tenant pinned to
//!   0.1.0 calls 0.1.0's function, never 0.2.0's, and a function only
//!   0.2.0 declares does not exist for it.
//! - **Loading a version beside the active one drops no public cache**
//!   (§1.14); an activation does.
//! - **A live feed closes with `version_changed`** when its tenant's pin
//!   moves (§1.4); a feed of a tenant that did not move stays open.
//!
//! A regression looks like a pinned tenant running the active version's
//! code, a tampered row served under its old name, a cold public cache
//! after a pin, or a feed that keeps signalling under the old version.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::BTreeMap;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use futures::StreamExt;
use ordo_conformance::{Fixture, Pg};
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::runtime::{self, CustodyChoice, RuntimeConfig};
use ordo_host::schema_apply::Program;
use ordo_host::tenants;
use ordo_host::versions::Versions;
use serde_json::{json, Value};
use tower::util::ServiceExt;

const SECRET: [u8; 32] = [11u8; 32];
const SPC_A: &str = "spc_a";
const SPC_B: &str = "spc_b";

/// 0.1.0: cards, a public notice board, and `stamp`, which says it is 0.1.0.
const V010: &str = "domain board @ 0.1.0

entity cards {
  title: text
}

entity notices {
  body: text(200) public
}

op addCard($title: text) =
  set cards { title: $title }

op cardList =
  cards[title]
  sort title
  take 50

op noticeList public =
  notices[body]
  sort body
  take 50

function stamp in board {
  call($text: text(200)) -> json by member
  code \"fn/stamp.js\"
}
";

/// 0.2.0: an optional field added, `stamp` says 0.2.0, and `only2`, which
/// 0.1.0 does not declare.
const V020: &str = "domain board @ 0.2.0

entity cards {
  title: text
  due:   int?
}

entity notices {
  body: text(200) public
}

op addCard($title: text) =
  set cards { title: $title }

op cardList =
  cards[title]
  sort title
  take 50

op noticeList public =
  notices[body]
  sort body
  take 50

function stamp in board {
  call($text: text(200)) -> json by member
  code \"fn/stamp.js\"
}

function only2 in board {
  call($text: text(200)) -> json by member
  code \"fn/only2.js\"
}
";

/// 0.3.0: another optional field, the same functions as 0.2.0.
const V030: &str = "domain board @ 0.3.0

entity cards {
  title: text
  due:   int?
  note:  text?
}

entity notices {
  body: text(200) public
}

op addCard($title: text) =
  set cards { title: $title }

op cardList =
  cards[title]
  sort title
  take 50

op noticeList public =
  notices[body]
  sort body
  take 50

function stamp in board {
  call($text: text(200)) -> json by member
  code \"fn/stamp.js\"
}

function only2 in board {
  call($text: text(200)) -> json by member
  code \"fn/only2.js\"
}
";

fn code(said: &str) -> String {
    format!("export default {{ async call(input, ctx) {{ return {{ from: \"{said}\" }}; }} }}")
}

fn pg_cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 8,
        ..PoolConfig::from_env()
    }
}

/// `ordo apply` for `source`, with its function code stored by hash.
async fn apply(pool: &Pool, source: &str, files: &[(&str, String)]) {
    let mut program = Program::compile(source).expect("compiles");
    let mut hashes = BTreeMap::new();
    for (path, text) in files {
        let h = ordo_host::functions::store_code(pool, text.as_bytes())
            .await
            .expect("code stored");
        hashes.insert(path.to_string(), h);
    }
    program.bind_code(&hashes).expect("code bound");
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

fn v010_files() -> Vec<(&'static str, String)> {
    vec![("fn/stamp.js", code("0.1.0"))]
}

fn v020_files() -> Vec<(&'static str, String)> {
    vec![
        ("fn/stamp.js", code("0.2.0")),
        ("fn/only2.js", code("only2")),
    ]
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
        page_key: ordo_api::PageKey::from_secret(&[12u8; 32]),
        checkpoint: None,
        security_checkpoint: false,
        apex_domain: None,
        cors: None,
        blob: None,
        exit_bundles: None,
        import_bundles: None,
        replica: "versions-loaded-test".into(),
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

/// One running node, and the handles the test reads it through.
struct Node {
    app: axum::Router,
    pool: Pool,
    db: String,
    deployment: std::sync::Arc<ordo_host::deploy::Deployment>,
    functions: std::sync::Arc<ordo_host::functions::HostFunctions>,
    public: ordo_api::PublicPath,
    serving: tokio::task::JoinHandle<()>,
}

impl Drop for Node {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

/// 0.1.0 then 0.2.0 applied, 0.2.0 active, two tenants each with its
/// owner (`ada` in `spc_a`, `bo` in `spc_b`), and a node serving them.
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
    apply(&pool, V010, &v010_files()).await;
    apply(&pool, V020, &v020_files()).await;
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
    let rt = runtime::assemble(config(&pg.db)).await.expect("assembles");
    // Each tenant's custody, under the node's own key provider.
    for space in [SPC_A, SPC_B] {
        let _ = rt.keys.create_space(&ordo_crypto::SpaceId(space.into()));
    }
    let app = runtime::door(&rt);
    let deployment = rt.deployment.clone();
    let functions = rt.functions.clone();
    let public = rt.api.public.clone();
    let serving = tokio::spawn(async move {
        let _ = runtime::serve(rt).await;
    });
    let node = Node {
        app,
        pool,
        db: pg.db.clone(),
        deployment,
        functions,
        public,
        serving,
    };
    // The node is up once its pins are read and the active version's
    // functions are in.
    wait(|| node.functions.table_versions()[0] == "0.2.0").await;
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
    headers: axum::http::HeaderMap,
    body: Value,
}

async fn call(
    node: &Node,
    method: &str,
    uri: &str,
    space: Option<&str>,
    who: Option<&str>,
    body: Option<Value>,
) -> Answer {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(s) = space {
        req = req.header("x-ordo-space", s);
    }
    if let Some(w) = who {
        req = req.header("authorization", format!("Bearer {}", mint(w)));
    }
    let req = match body {
        Some(v) => req
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
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

async fn stamp(node: &Node, function: &str, space: &str, who: &str) -> Answer {
    call(
        node,
        "POST",
        &format!("/fn/{function}"),
        Some(space),
        Some(who),
        Some(json!({ "text": "hi" })),
    )
    .await
}

async fn pin(node: &Node, space: &str, version: &str) {
    Versions::on(&node.pool)
        .pin(space, version, "deployment-owner:ada", 50)
        .await
        .expect("pinned");
}

/// **One function table per loaded version** (§1.10). `spc_a` is pinned
/// to 0.1.0 and calls 0.1.0's `stamp`; `spc_b` stays on 0.2.0 and calls
/// 0.2.0's. `only2`, which 0.1.0 does not declare, does not exist for
/// `spc_a`, and its answer is the one an unknown function gets.
#[tokio::test(flavor = "multi_thread")]
async fn a_pinned_tenant_calls_its_own_versions_functions_and_never_the_next() {
    let node = boot("ordo_vloaded_fn").await;
    let a = stamp(&node, "stamp", SPC_A, "ada").await;
    assert_eq!(a.status, 200, "{}", a.body);
    assert_eq!(a.body["result"]["from"], "0.2.0", "before the pin");

    pin(&node, SPC_A, "0.1.0").await;
    wait(|| {
        node.functions
            .table_versions()
            .contains(&"0.1.0".to_string())
    })
    .await;

    let a = stamp(&node, "stamp", SPC_A, "ada").await;
    assert_eq!(a.status, 200, "{}", a.body);
    assert_eq!(
        a.body["result"]["from"], "0.1.0",
        "the pinned version's code ran"
    );
    let b = stamp(&node, "stamp", SPC_B, "bo").await;
    assert_eq!(b.status, 200, "{}", b.body);
    assert_eq!(
        b.body["result"]["from"], "0.2.0",
        "the active version's code ran"
    );

    // The negative: 0.2.0's function never answers for a tenant on 0.1.0,
    // and the answer is the unknown function's.
    let missing = stamp(&node, "only2", SPC_A, "ada").await;
    let unknown = stamp(&node, "noSuchFunction", SPC_A, "ada").await;
    assert_ne!(missing.status, 200, "{}", missing.body);
    assert_eq!(missing.status, unknown.status);
    assert_eq!(missing.body["error"]["code"], unknown.body["error"]["code"]);
    let b = stamp(&node, "only2", SPC_B, "bo").await;
    assert_eq!(b.status, 200, "{}", b.body);

    // The run is recorded as the version that ran it.
    let runs = ordo_host::functions::runs(&node.pool, SPC_A).await.unwrap();
    assert!(
        runs.iter()
            .any(|r| r.principal.starts_with("function:stamp@0.1.0")),
        "{:?}",
        runs.iter().map(|r| r.principal.clone()).collect::<Vec<_>>()
    );

    // Moved back: the table no pin names is dropped.
    pin(&node, SPC_A, "0.2.0").await;
    wait(|| node.functions.table_versions() == vec!["0.2.0".to_string()]).await;
    let a = stamp(&node, "stamp", SPC_A, "ada").await;
    assert_eq!(a.body["result"]["from"], "0.2.0", "{}", a.body);
}

/// **Digest at load** (§6 conflict 4). 0.1.0's stored source is changed
/// by hand. A load of it is refused with the typed refusal naming the
/// version, the tenant pinned to it is closed with a plain answer, and the
/// other tenant is served on.
#[tokio::test(flavor = "multi_thread")]
async fn a_stored_version_whose_fingerprint_moved_is_refused_by_name() {
    let node = boot("ordo_vloaded_digest").await;
    // Untouched, it loads.
    let keys = Fixture::from_source("", "").expect("empty").keys.clone();
    let space = ordo_crypto::SpaceId("system".into());
    let cfg = pg_cfg(&node.db);
    ordo_host::deploy::load_pinned(&node.pool, &cfg, "0.1.0", keys.clone(), space.clone())
        .await
        .expect("an untouched row loads");

    node.pool
        .rows(
            "UPDATE \"$artifacts\" SET \"source\" = \"source\" || E'\\n# changed by hand' \
             WHERE \"version\" = '0.1.0'",
        )
        .await
        .expect("tampered");
    let refused =
        ordo_host::deploy::load_pinned(&node.pool, &cfg, "0.1.0", keys.clone(), space.clone())
            .await
            .expect_err("refused");
    assert!(
        matches!(&refused, ordo_host::deploy::LoadRefusal::Digest { version, .. } if version == "0.1.0"),
        "{refused}"
    );
    assert!(refused.to_string().contains("`0.1.0`"), "{refused}");
    assert!(
        refused.to_string().contains("ordo versions pin"),
        "{refused}"
    );
    let refused = ordo_host::deploy::load_version(&node.pool, &cfg, "0.1.0", keys, space)
        .await
        .expect_err("refused as a swap too");
    assert!(matches!(
        refused,
        ordo_host::deploy::LoadRefusal::Digest { .. }
    ));

    // The node: the tenant pinned to it is closed, the other served.
    pin(&node, SPC_A, "0.1.0").await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(node.deployment.loaded("0.1.0").is_none(), "never served");
    let a = call(&node, "GET", "/op/cardList", Some(SPC_A), Some("ada"), None).await;
    assert_eq!(a.status, 503, "{}", a.body);
    assert!(!a.body.to_string().contains("0.1.0"), "{}", a.body);
    let b = call(&node, "GET", "/op/cardList", Some(SPC_B), Some("bo"), None).await;
    assert_eq!(b.status, 200, "{}", b.body);
}

/// **The public cache belongs to the deployment's own space** (§1.14).
/// A tenant pinned beside the active version loads 0.1.0 on this node,
/// and the public answer is still a hit. An activation changes the
/// deployment's own space's version, and the cache drops.
#[tokio::test(flavor = "multi_thread")]
async fn loading_a_version_beside_the_active_one_drops_no_public_cache() {
    let node = boot("ordo_vloaded_public").await;
    let first = call(&node, "GET", "/op/noticeList", None, None, None).await;
    assert_eq!(first.status, 200, "{}", first.body);
    let cache = |a: &Answer| {
        a.headers
            .get("x-ordo-cache")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let again = call(&node, "GET", "/op/noticeList", None, None, None).await;
    assert_eq!(cache(&again), "hit");
    let held = node.public.size();

    pin(&node, SPC_A, "0.1.0").await;
    wait(|| node.deployment.loaded("0.1.0").is_some()).await;
    wait(|| {
        node.functions
            .table_versions()
            .contains(&"0.1.0".to_string())
    })
    .await;
    assert_eq!(node.public.size(), held, "nothing dropped");
    let after = call(&node, "GET", "/op/noticeList", None, None, None).await;
    assert_eq!(cache(&after), "hit", "a pin beside the active version");

    // The deployment's own space moves: everything drops.
    apply(&node.pool, V030, &v020_files()).await;
    wait(|| node.deployment.current().version == "0.3.0").await;
    // The swap sets the version a moment before it drops the cache (seen on
    // CI, PR 37), so the first answer after it may still be a hit. Each
    // read before the drop is a hit; the first after it is a miss.
    let mut seen = Vec::new();
    for _ in 0..50 {
        let moved = call(&node, "GET", "/op/noticeList", None, None, None).await;
        seen.push(cache(&moved));
        if seen.last().map(String::as_str) == Some("miss") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        seen.last().map(String::as_str),
        Some("miss"),
        "an activation drops the cache: {seen:?}"
    );
}

/// Read SSE frames until one named `event`, or `limit` passes. Answers
/// the frame's data.
async fn next_event(
    frames: &mut (impl futures::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin),
    buf: &mut String,
    event: &str,
    limit: Duration,
) -> Option<String> {
    let until = tokio::time::Instant::now() + limit;
    loop {
        while let Some(end) = buf.find("\n\n") {
            let frame: String = buf.drain(..end + 2).collect();
            let name = frame
                .lines()
                .find_map(|l| l.strip_prefix("event:"))
                .map(|s| s.trim().to_string());
            if name.as_deref() == Some(event) {
                let data = frame
                    .lines()
                    .filter_map(|l| l.strip_prefix("data:"))
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join("\n");
                return Some(data);
            }
        }
        let left = until.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        match tokio::time::timeout(left, frames.next()).await {
            Ok(Some(Ok(chunk))) => buf.push_str(&String::from_utf8_lossy(&chunk)),
            Ok(_) => return None,
            Err(_) => return None,
        }
    }
}

async fn open_feed(
    node: &Node,
    space: &str,
    who: &str,
) -> impl futures::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin {
    let req = Request::builder()
        .method("GET")
        .uri("/op/cardList?live=1")
        .header("x-ordo-space", space)
        .header("authorization", format!("Bearer {}", mint(who)))
        .body(Body::empty())
        .unwrap();
    let resp = node.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    resp.into_body().into_data_stream()
}

/// **Stream close on a pin switch** (§1.4). A feed `spc_a` opened under
/// 0.2.0 ends with a `version_changed` frame as soon as its pin moves to
/// 0.1.0, well before the 30-second re-check. The frame names no version.
/// `spc_b`'s feed, whose tenant did not move, stays open.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_feed_closes_with_version_changed_when_its_tenants_pin_moves() {
    let node = boot("ordo_vloaded_feed").await;
    let mut a = open_feed(&node, SPC_A, "ada").await;
    let mut b = open_feed(&node, SPC_B, "bo").await;
    let (mut abuf, mut bbuf) = (String::new(), String::new());

    pin(&node, SPC_A, "0.1.0").await;
    let data = next_event(
        &mut a,
        &mut abuf,
        "version_changed",
        Duration::from_secs(10),
    )
    .await
    .expect("the moved tenant's feed ends with version_changed");
    let body: Value = serde_json::from_str(&data).expect("an envelope");
    assert_eq!(body["error"]["code"], "space_unavailable", "{body}");
    assert!(!data.contains("0.1.0") && !data.contains("0.2.0"), "{data}");
    assert!(
        next_event(&mut a, &mut abuf, "invalidate", Duration::from_secs(1))
            .await
            .is_none(),
        "nothing after the close"
    );

    // The tenant that did not move: its feed still carries signals.
    let w = call(
        &node,
        "POST",
        "/op/addCard",
        Some(SPC_B),
        Some("bo"),
        Some(json!({ "title": "still here" })),
    )
    .await;
    assert_eq!(w.status, 200, "{}", w.body);
    assert!(
        next_event(&mut b, &mut bbuf, "invalidate", Duration::from_secs(10))
            .await
            .is_some(),
        "the unmoved tenant's feed is still open"
    );
    assert!(!bbuf.contains("version_changed"), "{bbuf}");
}
