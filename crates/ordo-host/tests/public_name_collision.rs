//! An op named like an entity, through the real door (reported fix,
//! `examples/note-viewer`, 2026-11-19).
//!
//! What was seen: `op publicNotes public` read `external publicNotes` with a
//! filter (`tags contains "public"`), and a visitor with no token got every
//! row of the table. `ordo plan` printed the filtered SQL.
//!
//! The cause: the door calls an op by building the call form `name(args)`
//! and binding it, and the binder resolved a source name to an entity before
//! it looked for an op. A name that was both answered the entity's own read,
//! with none of the op's conditions, projection, sort or take.
//!
//! The check now refuses an op named like an entity (`op_named_like_entity`),
//! so a program with the collision does not compile. These tests build the
//! collision the check would refuse, the way a program compiled before the
//! refusal holds it: the op is registered again under the entity's name. A
//! regression looks like a row the op's filter excludes in the answer.
//!
//! Needs the conformance cluster. Its own databases, prefixed
//! `ordo_collide_`.

use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use ordo_conformance::{fixture_space, Fixture, Pg};
use ordo_host::runtime::{assemble_embedded, door, CustodyChoice, RuntimeConfig};
use serde_json::Value;
use tower::util::ServiceExt;

/// A native public entity and an external one, each read by a public op
/// that keeps only some rows. The ops are written under other names; the
/// test registers each again under its entity's name.
const PROGRAM: &str = r#"
entity notices {
  public
  body: text
  shown: bool
}

entity plans {
  name:   text public
  margin: int
}

external published (key: pub) from crm.posts shared {
  public
  id <- "post_id"
  title: text <- "title"
  open: bool <- "open"
}

op noticesOp public =
  notices(shown == true)[body]
  sort body
  take 50

op plansOp public =
  plans[name]
  sort name
  take 50

op publishedOp public =
  published(open == true)[title]
  sort title
  take 50
"#;

const SEED: &str = r#"
INSERT INTO "members" ("$space", "id", "principal", "displayName", "kind", "memberCount", "role", "status", "joined") VALUES
  ('system', 'mem_p', 'mem_p', 'Pat', 'principal', 1, 'owner', 'active', DATE '2026-01-01');
INSERT INTO "notices" ("$space", "id", "rev", "body", "shown") VALUES
  ('system', 'notice_1', 1, 'visible notice', true),
  ('system', 'notice_2', 1, 'hidden notice', false);
INSERT INTO "plans" ("$space", "id", "rev", "name", "margin") VALUES
  ('system', 'plan_1', 1, 'basic', 4242);
DROP SCHEMA IF EXISTS crm CASCADE;
CREATE SCHEMA crm;
CREATE TABLE crm.posts (post_id bigint PRIMARY KEY, title text NOT NULL, open boolean NOT NULL);
INSERT INTO crm.posts VALUES (1, 'open post', true), (2, 'private post', false);
"#;

fn free_addr() -> std::net::SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// The program, with each op registered again under its entity's name: the
/// collision `op_named_like_entity` refuses at check time.
fn colliding() -> Fixture {
    let mut f = Fixture::from_source(PROGRAM, SEED).expect("the program compiles");
    let mut renamed = Vec::new();
    for (op, entity) in [
        ("noticesOp", "notices"),
        ("plansOp", "plans"),
        ("publishedOp", "published"),
    ] {
        let mut decl = f.registry.get(op).expect("declared").clone();
        decl.name.name = entity.to_string();
        renamed.push(decl);
    }
    f.registry.extend_from(&ordo_syntax::Program {
        ops: renamed,
        ..ordo_syntax::Program::default()
    });
    f
}

async fn boot(stem: &str) -> (ordo_host::runtime::Runtime, axum::Router) {
    let pg = Pg::scoped(stem);
    let f = colliding();
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
        tightened: Vec::new(),
        custody: CustodyChoice::Test,
        token_secret: [7u8; 32],
        page_key: ordo_api::PageKey::from_secret(&[8u8; 32]),
        checkpoint: None,
        security_checkpoint: false,
        apex_domain: None,
        cors: None,
        blob: None,
        exit_bundles: None,
        import_bundles: None,
        replica: "public-name-collision-test".into(),
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
    };
    let rt = assemble_embedded(cfg, colliding()).await.expect("boots");
    let app = door(&rt);
    (rt, app)
}

/// A visitor: no token, no space.
async fn visit(app: &axum::Router, uri: &str) -> (u16, String, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body).to_string();
    let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, text, json)
}

fn column(body: &Value, field: &str) -> Vec<String> {
    body["rows"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|r| r[field].as_str().unwrap_or("").to_string())
        .collect()
}

/// The host compiles a stored version with this same call, so a version
/// holding the collision is refused at load as well as at deploy.
#[test]
fn the_compile_refuses_an_op_named_like_an_entity() {
    for (entity, op) in [
        (
            "notices",
            "op notices public =\n  notices(shown == true)[body]\n  take 50\n",
        ),
        ("plans", "op plans public =\n  plans[name]\n  take 50\n"),
        (
            "published",
            "op published public =\n  published(open == true)[title]\n  take 50\n",
        ),
    ] {
        let src = format!("{PROGRAM}\n{op}");
        let e = Fixture::from_source(&src, SEED)
            .err()
            .unwrap_or_else(|| panic!("`op {entity}` beside `{entity}` compiled"));
        assert!(e.contains("op_named_like_entity"), "{e}");
        assert!(e.contains(&format!("`{entity}`")), "{e}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_visitor_gets_only_the_rows_the_op_keeps() {
    let (_rt, app) = boot("ordo_collide_public").await;

    // A native public entity.
    let (status, text, body) = visit(&app, "/op/notices").await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(column(&body, "body"), ["visible notice"], "{text}");
    assert!(!text.contains("hidden notice"), "{text}");
    // The op's projection, not the entity's fields.
    assert!(!text.contains("\"shown\""), "{text}");

    // A private field of a native entity whose other fields are public.
    let (status, text, body) = visit(&app, "/op/plans").await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(column(&body, "name"), ["basic"], "{text}");
    assert!(!text.contains("margin") && !text.contains("4242"), "{text}");

    // An external entity, the shape the note viewer had.
    let (status, text, body) = visit(&app, "/op/published").await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(column(&body, "title"), ["open post"], "{text}");
    assert!(!text.contains("private post"), "{text}");
    assert!(!text.contains("\"open\""), "{text}");

    // The ops under their own names answer the same rows.
    let (_, _, a) = visit(&app, "/op/noticesOp").await;
    assert_eq!(column(&a, "body"), ["visible notice"]);
    let (_, _, a) = visit(&app, "/op/publishedOp").await;
    assert_eq!(column(&a, "title"), ["open post"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_signed_in_caller_gets_only_the_rows_the_op_keeps() {
    let (_rt, app) = boot("ordo_collide_signed_in").await;
    let token = ordo_identity::mint(
        &ordo_identity::TokenKey::from_secret(&[7u8; 32]),
        &ordo_identity::TokenSpec {
            principal: ordo_identity::principal::PrincipalId("mem_p".into()),
            expires: ordo_identity::principal::Timestamp(u64::MAX / 2),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    );
    for (uri, field, kept, dropped) in [
        ("/op/notices", "body", "visible notice", "hidden notice"),
        ("/op/published", "title", "open post", "private post"),
    ] {
        let req = Request::builder()
            .method("GET")
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .header("x-ordo-space", "system")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes).to_string();
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        assert_eq!(status, 200, "{uri}: {text}");
        assert_eq!(column(&body, field), [kept], "{uri}: {text}");
        assert!(!text.contains(dropped), "{uri}: {text}");
    }
}
