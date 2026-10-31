//! The public path, phase 1 (`design-public-path.md` §11 rows 1, 3 and 4),
//! through the router the server serves (`runtime::door`) and real Postgres.
//!
//! - **Row 1, the anonymous door.** A tokenless call to a public read runs as
//!   `$anonymous` in the deployment's space; a tokenless call to anything
//!   else, or in another space, is the door's one answer, byte for byte.
//! - **Row 3, the cache.** A second identical read is a hit; a write to an
//!   entity the op reads makes the next read a miss and fresh; a write to an
//!   unrelated entity leaves the hit; a deploy drops everything; a lost
//!   listener drops everything and nothing is stored until it is heard again.
//! - **Row 4, the CDN headers.** `Cache-Control` carries the settings, the
//!   `ETag` answers `304`, and `Cache-Tag` names the op and its entities.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use ordo_conformance::{fixture_space, Fixture, Pg};
use ordo_host::runtime::{assemble_embedded, door, CustodyChoice, RuntimeConfig};
use serde_json::Value;
use tower::util::ServiceExt;

const PROGRAM: &str = "
entity plans {
  name:   text  public
  price:  money public
  margin: money
}

entity notices {
  body: text public
}

op pricing public =
  plans(price > 0)[name, price]
  sort name
  take 50

op margins =
  plans[name, margin]
  sort name
  take 50

op noticeList public =
  notices[body]
  sort body
  take 50

op addPlan($name: text(80), $price: money, $margin: money) =
  set plans { name: $name, price: $price, margin: $margin }

op addNotice($body: text(80)) =
  set notices { body: $body }
";

/// `mem_p` owns the deployment's space (`system`), where public data lives.
/// `mem_1` is a member of the tenant `conformance`, which holds a plan of
/// its own that no visitor may see.
const SEED: &str = r#"
INSERT INTO "members" ("$space", "id", "principal", "displayName", "kind", "memberCount", "role", "status", "joined") VALUES
  ('system', 'mem_p', 'mem_p', 'Pat', 'principal', 1, 'owner', 'active', DATE '2026-01-01'),
  (@SPACE@, 'mem_1', 'mem_1', 'Ana', 'principal', 1, 'owner', 'active', DATE '2026-01-01');
INSERT INTO "plans" ("$space", "id", "rev", "name", "price", "margin") VALUES
  ('system', 'plan_1', 1, 'basic', 500, 100),
  ('system', 'plan_2', 1, 'pro', 2000, 900),
  (@SPACE@, 'plan_9', 1, 'tenant-only', 7, 7);
"#;

const SECRET: [u8; 32] = [7u8; 32];

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

struct Answer {
    status: u16,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    headers: &[(&str, String)],
    body: Option<Value>,
) -> Answer {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let req = match body {
        Some(v) => req
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap()
        .to_vec();
    Answer {
        status,
        headers,
        body,
    }
}

fn free_addr() -> std::net::SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

async fn boot(stem: &str) -> (ordo_host::runtime::Runtime, axum::Router) {
    let pg = Pg::scoped(stem);
    let f = Fixture::from_source(PROGRAM, SEED).expect("the public program compiles");
    f.reset(&pg).expect("seeded");
    let cfg = RuntimeConfig {
        blob_routing: ordo_host::runtime::BlobRouting::Single,
        pg: ordo_exec::PoolConfig {
            db: pg.db.clone(),
            size: 8,
            ..ordo_exec::PoolConfig::from_env()
        },
        addr: free_addr(),
        shutdown_grace: Duration::from_secs(1),
        space: fixture_space(),
        custody: CustodyChoice::Test,
        token_secret: SECRET,
        page_key: ordo_api::PageKey::from_secret(&[8u8; 32]),
        checkpoint: None,
        security_checkpoint: false,
        apex_domain: None,
        cors: None,
        blob: None,
        exit_bundles: None,
        import_bundles: None,
        replica: "public-path-test".into(),
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
    };
    let f = Fixture::from_source(PROGRAM, SEED).unwrap();
    let rt = assemble_embedded(cfg, f).await.expect("boots");
    let app = door(&rt);
    (rt, app)
}

/// A visitor: no token, no space.
async fn visit(app: &axum::Router, uri: &str) -> Answer {
    call(app, "GET", uri, &[], None).await
}

/// The owner of the deployment's space writes a row there.
async fn write_as_owner(app: &axum::Router, op: &str, body: Value) {
    let a = call(
        app,
        "POST",
        &format!("/op/{op}"),
        &[
            ("authorization", format!("Bearer {}", mint("mem_p"))),
            ("x-ordo-space", "system".to_string()),
        ],
        Some(body),
    )
    .await;
    assert_eq!(a.status, 200, "{op}: {}", String::from_utf8_lossy(&a.body));
}

/// Read until the cache says `miss`: the invalidation rides `NOTIFY`, so it
/// lands a moment after the write commits.
async fn until_miss(app: &axum::Router, uri: &str) -> Answer {
    for _ in 0..100 {
        let a = visit(app, uri).await;
        if a.header("x-ordo-cache") == "miss" {
            return a;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{uri} never missed after the write");
}

fn names(a: &Answer) -> Vec<String> {
    a.json()["rows"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|r| r["name"].as_str().unwrap_or("").to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_anonymous_door() {
    let (_rt, app) = boot("ordo_public_door").await;

    // tokenless_public_read_is_served
    let a = visit(&app, "/op/pricing").await;
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    assert_eq!(names(&a), ["basic", "pro"], "the deployment's rows only");
    let text = String::from_utf8_lossy(&a.body);
    assert!(
        !text.contains("margin"),
        "a private field is never served: {text}"
    );
    assert!(
        !text.contains("tenant-only"),
        "a tenant's row is never served"
    );

    // Naming the deployment's space is the same call.
    let named = call(
        &app,
        "GET",
        "/op/pricing",
        &[("x-ordo-space", "system".into())],
        None,
    )
    .await;
    assert_eq!(named.status, 200);

    // tokenless_non_public_op_answers_byte_identical_access_denied
    let unknown = call(
        &app,
        "GET",
        "/op/noSuchOp",
        &[("x-ordo-space", "system".into())],
        None,
    )
    .await;
    assert_eq!(unknown.status, 403);
    let private = call(
        &app,
        "GET",
        "/op/margins",
        &[("x-ordo-space", "system".into())],
        None,
    )
    .await;
    assert_eq!(private.status, 403);
    // The door's denial names the op asked for and nothing else, so the
    // bytes are what a tokenless call got before the public path existed.
    let denial = |op: &str| {
        format!(
            r#"{{"error":{{"code":"access_denied","message":"missing bearer token","op":"{op}"}}}}"#
        )
        .into_bytes()
    };
    assert_eq!(unknown.body, denial("noSuchOp"));
    assert_eq!(
        private.body,
        denial("margins"),
        "one answer for unknown and private"
    );
    assert_eq!(private.json()["error"]["code"], "access_denied");

    // a_space_other_than_the_deployments_is_refused_identically
    let other = call(
        &app,
        "GET",
        "/op/pricing",
        &[("x-ordo-space", fixture_space().0)],
        None,
    )
    .await;
    assert_eq!(other.status, 403);
    assert_eq!(
        other.body,
        denial("pricing"),
        "a tenant is never reachable without a token"
    );

    // A tokenless write is the door's answer too: public writes are not built.
    let write = call(
        &app,
        "POST",
        "/op/addNotice",
        &[("x-ordo-space", "system".into())],
        Some(serde_json::json!({ "body": "hi" })),
    )
    .await;
    assert_eq!(write.status, 403);
    assert_eq!(write.body, denial("addNotice"));

    // refine_on_a_public_op_is_refused
    let refined = visit(&app, "/op/pricing?$refine=%7B%22where%22%3A%5B%5D%7D").await;
    assert_eq!(
        refined.status,
        400,
        "{}",
        String::from_utf8_lossy(&refined.body)
    );
    assert_eq!(refined.json()["error"]["code"], "invalid_refinement");

    // A member calling the public op with a token takes the ordinary path.
    let member = call(
        &app,
        "GET",
        "/op/pricing",
        &[
            ("authorization", format!("Bearer {}", mint("mem_1"))),
            ("x-ordo-space", fixture_space().0),
        ],
        None,
    )
    .await;
    assert_eq!(
        member.status,
        200,
        "{}",
        String::from_utf8_lossy(&member.body)
    );
    assert_eq!(
        member.header("x-ordo-cache"),
        "",
        "members do not use the public cache"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cache_and_its_invalidation() {
    let (rt, app) = boot("ordo_public_cache").await;

    // second_identical_read_is_a_hit
    let first = visit(&app, "/op/pricing").await;
    assert_eq!(first.header("x-ordo-cache"), "miss");
    let second = visit(&app, "/op/pricing").await;
    assert_eq!(second.header("x-ordo-cache"), "hit");
    assert_eq!(second.body, first.body);

    // a_write_to_an_unrelated_entity_leaves_the_hit
    write_as_owner(&app, "addNotice", serde_json::json!({ "body": "hello" })).await;
    let _ = until_miss(&app, "/op/noticeList").await;
    let still = visit(&app, "/op/pricing").await;
    assert_eq!(still.header("x-ordo-cache"), "hit");

    // a_write_to_a_read_entity_makes_the_next_read_a_miss_and_fresh
    write_as_owner(
        &app,
        "addPlan",
        serde_json::json!({ "name": "team", "price": "900", "margin": "300" }),
    )
    .await;
    let fresh = until_miss(&app, "/op/pricing").await;
    assert_eq!(names(&fresh), ["basic", "pro", "team"]);
    assert_eq!(
        visit(&app, "/op/pricing").await.header("x-ordo-cache"),
        "hit"
    );

    // a_deploy_drops_everything
    let live = rt.deployment.current();
    ordo_host::public::after_deploy(&rt.pg_bus(), &rt.api.public, &live.fixture);
    assert_eq!(rt.api.public.size().0, 0);
    assert_eq!(
        visit(&app, "/op/pricing").await.header("x-ordo-cache"),
        "miss"
    );
    assert_eq!(
        visit(&app, "/op/pricing").await.header("x-ordo-cache"),
        "hit"
    );

    // a_lost_listener_drops_everything_on_reconnect
    let bus = rt.pg_bus();
    bus.lose_for_test();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rt.api.public.size().0, 0, "a loss drops every entry");
    let deaf = visit(&app, "/op/pricing").await;
    assert_eq!(deaf.header("x-ordo-cache"), "miss");
    assert_eq!(
        deaf.header("cache-control"),
        "public, max-age=0, s-maxage=0",
        "no shared cache may hold an answer while the listener is lost"
    );
    assert_eq!(
        visit(&app, "/op/pricing").await.header("x-ordo-cache"),
        "miss",
        "nothing is stored while deaf"
    );
    bus.hear_for_test();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        visit(&app, "/op/pricing").await.header("x-ordo-cache"),
        "miss"
    );
    assert_eq!(
        visit(&app, "/op/pricing").await.header("x-ordo-cache"),
        "hit"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cdn_headers() {
    let (_rt, app) = boot("ordo_public_headers").await;

    // headers_carry_the_settings
    let a = visit(&app, "/op/pricing").await;
    assert_eq!(
        a.header("cache-control"),
        "public, max-age=0, s-maxage=60, stale-while-revalidate=300"
    );
    assert_eq!(a.header("vary"), "Accept");
    let tags = a.header("cache-tag");
    assert!(tags.contains("op:pricing"), "{tags}");
    assert!(tags.contains("entity:plans"), "{tags}");
    assert_eq!(a.header("surrogate-key"), tags.replace(", ", " "));
    let etag = a.header("etag").to_string();
    assert!(etag.starts_with('"') && etag.len() == 66, "{etag}");

    // The ETag answers 304 with no body, hit or miss.
    let b = call(
        &app,
        "GET",
        "/op/pricing",
        &[("if-none-match", etag.clone())],
        None,
    )
    .await;
    assert_eq!(b.status, 304);
    assert!(b.body.is_empty());
    assert_eq!(b.header("etag"), etag);
}

/// Misses past `ORDO_PUBLIC_MISSES_PER_MINUTE` are refused `429` and name the
/// setting. Hits are not counted, and the cap is per op (audit 2026-10-11,
/// finding 27).
#[tokio::test(flavor = "multi_thread")]
async fn public_misses_past_the_cap_are_refused_by_name() {
    let (rt, _) = boot("ordo_public_misscap").await;
    let with = |settings: ordo_api::PublicSettings| {
        let mut api = rt.api.clone();
        api.public = ordo_api::PublicPath::new(settings, None);
        ordo_api::router(api)
    };

    // With the cache on, one miss then hits: the cap is never reached.
    let app = with(ordo_api::PublicSettings {
        misses_per_minute: 1,
        ..Default::default()
    });
    assert_eq!(
        visit(&app, "/op/pricing").await.header("x-ordo-cache"),
        "miss"
    );
    for _ in 0..5 {
        let hit = visit(&app, "/op/pricing").await;
        assert_eq!(hit.header("x-ordo-cache"), "hit", "a hit is not a miss");
    }

    // With the cache off, every call is a miss, and the third is over.
    let app = with(ordo_api::PublicSettings {
        misses_per_minute: 2,
        cache_bytes: 0,
        ..Default::default()
    });
    for _ in 0..2 {
        assert_eq!(visit(&app, "/op/pricing").await.status, 200);
    }
    let over = visit(&app, "/op/pricing").await;
    let body = String::from_utf8_lossy(&over.body).to_string();
    assert_eq!(over.status, 429, "{body}");
    assert!(body.contains("ORDO_PUBLIC_MISSES_PER_MINUTE"), "{body}");
    assert!(body.contains("quota_exceeded"), "{body}");
    assert_eq!(visit(&app, "/op/noticeList").await.status, 200, "per op");
}
