//! External entities end to end (ruled 2026-11-15, `external-entities`):
//! a table another system owns, created here by raw SQL in its own schema,
//! declared as an `external`, applied, and read through the real door.
//!
//! What it proves:
//!
//! - `std.patrons.list` and `std.patrons.get` read the foreign rows, ids
//!   show as `pat_<key>`, and paging sorts by the raw key (`pat_42`,
//!   `pat_10`, `pat_9`, which text order would get wrong).
//! - A developer op joins a native entity to the external one, and a ref to
//!   a row that is not there reads as null rather than failing. The seed
//!   writes that dangling ref directly, which only works because no foreign
//!   key was made.
//! - `?live=1` on an op that reads an external is refused `not_live`.
//! - The apply created nothing in `crm`: the foreign table is exactly what
//!   the seed made.
//!
//! A regression looks like a `$space` predicate on the foreign table (every
//! read fails), ids paged in text order, a cast error from a malformed id,
//! or a `CREATE`/`ALTER` reaching the other system's schema.
//!
//! Needs the conformance cluster. Its own databases, prefixed `ordo_ext_`.

mod refine_support;

use std::sync::Arc;

use axum::http::StatusCode;
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use refine_support::{call, door_over, path};
use serde_json::Value;

const PROGRAM: &str = r#"
external patrons (key: pat) from crm.customers shared {
  id <- "customer_id"
  name: text <- "full_name"
  email: text? <- email
}

entity tickets {
  customer -> patrons
  subject: text
}

op ticketsWithCustomer =
  tickets[subject, customer.name]
  sort subject

op patronByEmail($e: text) =
  patrons(email == $e)[id, name]

op mine =
  patrons(name == $caller)[id]
"#;

/// The other system's table, made the way that system would make it, and
/// two tickets: one pointing at a customer, one at a customer who is gone.
const FOREIGN: &str = r#"
DROP SCHEMA IF EXISTS crm CASCADE;
CREATE SCHEMA crm;
CREATE TABLE crm.customers (
  customer_id bigint PRIMARY KEY,
  full_name   text NOT NULL,
  email       text
);
INSERT INTO crm.customers VALUES
  (9,  'Ada', 'ada@example.com'),
  (10, 'Bo',  NULL),
  (42, 'Cy',  'cy@example.com');
INSERT INTO "tickets" ("$space", "id", "customer", "subject") VALUES
  (@SPACE@, 'tick_1', 'pat_9',  'a'),
  (@SPACE@, 'tick_2', 'pat_77', 'b');
"#;

async fn door(db: &str, page_size: i64) -> ordo_api::ApiState {
    let seed: &'static str = Box::leak(format!("{SEED}\n{FOREIGN}").into_boxed_str());
    let f = Arc::new(
        Fixture::from_source(&format!("{FIXTURE}\n{PROGRAM}"), seed)
            .expect("a program with an external compiles"),
    );
    let (state, _, _) = door_over(f, db, page_size, ordo_exec::PoolConfig::from_env(), |k| k).await;
    state
}

fn rows(body: &Value) -> Vec<Value> {
    body["rows"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn std_list_and_get_read_the_foreign_table_in_key_order() {
    let state = door("ordo_ext_std", 2).await;

    let (status, body) = call(&state, "GET", "/op/std.patrons.list", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let first: Vec<(&str, &str)> = body["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| (r["id"].as_str().unwrap(), r["name"].as_str().unwrap()))
        .collect();
    assert_eq!(first, [("pat_42", "Cy"), ("pat_10", "Bo")], "{body}");
    let next = body["next"].as_str().expect("a second page").to_string();
    let (status, body) = call(
        &state,
        "GET",
        &path("std.patrons.list", "", None, Some(&next)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids: Vec<&str> = body["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["pat_9"], "{body}");

    let (status, body) = call(&state, "GET", "/op/std.patrons.get?id=pat_10", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(rows(&body).len(), 1, "{body}");
    assert_eq!(body["rows"][0]["name"], "Bo");
    assert!(body["rows"][0]["email"].is_null(), "{body}");

    // `count` is a read like the other two, derived for every entity.
    let (status, body) = call(&state, "GET", "/op/std.patrons.count", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["n"], 3, "{body}");

    // Not this entity's id: no row, never a cast error.
    for bad in ["pat_x", "acct_10", "pat_99999999999999999999"] {
        let (status, body) = call(
            &state,
            "GET",
            &format!("/op/std.patrons.get?id={bad}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{bad}: {body}");
        assert!(rows(&body).is_empty(), "{bad}: {body}");
    }
}

/// The real write path (review finding 1): a native row whose ref names an
/// external is created through the door. The write checks the prefix and
/// the key's form only, never the other system's table, so a ref to a row
/// that is not there is accepted and later reads as missing.
#[tokio::test]
async fn a_native_row_may_ref_an_external_through_the_write_path() {
    let state = door("ordo_ext_write", 50).await;
    for (customer, subject) in [("pat_9", "c"), ("pat_77", "d")] {
        let (status, body) = call(
            &state,
            "POST",
            "/op/std.tickets.create",
            Some(serde_json::json!({ "customer": customer, "subject": subject })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{customer}: {body}");
    }
    // Not this external's id: refused before any SQL, as a missing row of
    // an Ordo entity would be.
    for bad in ["acct_9", "pat_x", "pat_9223372036854775808"] {
        let (status, body) = call(
            &state,
            "POST",
            "/op/std.tickets.create",
            Some(serde_json::json!({ "customer": bad, "subject": "e" })),
        )
        .await;
        assert_ne!(status, StatusCode::OK, "{bad}: {body}");
    }
    let (status, body) = call(&state, "GET", "/op/ticketsWithCustomer", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let got: Vec<(String, Value)> = rows(&body)
        .iter()
        .map(|r| {
            (
                r["subject"].as_str().unwrap().to_string(),
                r["name"].clone(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("a".to_string(), Value::from("Ada")),
            ("b".to_string(), Value::Null),
            ("c".to_string(), Value::from("Ada")),
            ("d".to_string(), Value::Null),
        ],
        "{body}"
    );
}

/// Review finding 4: an integer key past 18 digits is still found by get,
/// by a filter and by a join, and one past the bigint range is no row.
#[tokio::test]
async fn a_nineteen_digit_key_is_found_everywhere() {
    let state = door("ordo_ext_bigint", 50).await;
    Pg::named("ordo_ext_bigint")
        .exec("INSERT INTO crm.customers VALUES (1000000000000000000, 'Di', 'di@example.com')")
        .expect("inserts");
    let id = "pat_1000000000000000000";
    let (status, body) = call(&state, "GET", &format!("/op/std.patrons.get?id={id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["name"], "Di", "{body}");
    let (status, body) = call(
        &state,
        "GET",
        "/op/patronByEmail?e=di%40example.com",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["id"], id, "{body}");
    let (status, body) = call(
        &state,
        "POST",
        "/op/std.tickets.create",
        Some(serde_json::json!({ "customer": id, "subject": "z" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = call(&state, "GET", "/op/ticketsWithCustomer", None).await;
    let z = rows(&body)
        .into_iter()
        .find(|r| r["subject"] == "z")
        .expect("the new ticket");
    assert_eq!(z["name"], "Di", "the join decodes 19 digits: {body}");
    let (status, body) = call(
        &state,
        "GET",
        "/op/std.patrons.get?id=pat_9223372036854775808",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(rows(&body).is_empty(), "{body}");
}

#[tokio::test]
async fn a_developer_op_joins_a_native_entity_to_the_external() {
    let state = door("ordo_ext_join", 50).await;
    let (status, body) = call(&state, "GET", "/op/ticketsWithCustomer", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let got: Vec<(String, Value)> = rows(&body)
        .iter()
        .map(|r| {
            (
                r["subject"].as_str().unwrap().to_string(),
                r["name"].clone(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("a".to_string(), Value::from("Ada")),
            // `pat_77` is not in the foreign table: the ticket stays and
            // its customer reads as missing.
            ("b".to_string(), Value::Null),
        ],
        "{body}"
    );

    let (status, body) = call(
        &state,
        "GET",
        "/op/patronByEmail?e=cy%40example.com",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["id"], "pat_42", "{body}");
}

#[tokio::test]
async fn a_read_of_an_external_is_never_live() {
    let state = door("ordo_ext_live", 50).await;
    let (status, body) = call(&state, "GET", "/op/std.patrons.list?live=1", None).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(body.to_string().contains("not_live"), "{body}");
    // Review finding 2: an op the generic bind cannot settle (`$caller` in
    // a value position) is not live either. Unknown is not live.
    let (status, body) = call(&state, "GET", "/op/mine?live=1", None).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(body.to_string().contains("not_live"), "{body}");
    // A read of the native side alone stays live-capable.
    let (status, body) = call(&state, "GET", "/op/std.tickets.list", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn the_apply_leaves_the_foreign_schema_as_the_other_system_made_it() {
    let _state = door("ordo_ext_ddl", 50).await;
    let pg = Pg::named("ordo_ext_ddl");
    let out = pg
        .query(
            "SELECT string_agg(table_name || '.' || column_name, ',' ORDER BY table_name, column_name) \
             FROM information_schema.columns WHERE table_schema = 'crm'",
        )
        .expect("reads the catalog");
    assert_eq!(
        out[0][0].as_deref().unwrap_or(""),
        "customers.customer_id,customers.email,customers.full_name",
        "Ordo created or altered something in the other system's schema"
    );
    let fks = pg
        .query(
            "SELECT count(*) FROM pg_constraint WHERE contype = 'f' \
             AND conrelid = '\"tickets\"'::regclass",
        )
        .expect("reads the catalog");
    assert_eq!(
        fks[0][0].as_deref(),
        Some("0"),
        "a ref to an external takes no foreign key"
    );
}

/// Reported 2026-11-17 (`required-ref-to-external-refused`): the boot gate
/// asked for a foreign key on every required ref, and `tickets.customer`
/// has none because its target is external. A regression is a
/// `NeverNullGone` refusal naming `tickets$customer$fk`.
#[tokio::test]
async fn a_required_ref_to_an_external_passes_the_never_null_gate() {
    let _state = door("ordo_ext_gate", 50).await;
    let f = Fixture::from_source(&format!("{FIXTURE}\n{PROGRAM}"), "")
        .expect("a program with an external compiles");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: "ordo_ext_gate".into(),
        size: 1,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("connects");
    let gone = ordo_host::shape::never_null_gone(&pool, &f.schema)
        .await
        .expect("the catalog reads");
    assert!(gone.is_empty(), "{gone:?}");
}

// ── `public` externals on the public path ───────────────────────────────
//
// Ruled 2026-11-15 (`external-entities`, ask 5): an external may be
// `public`, and its derived reads then serve its public fields to a visitor
// with no token through an authored `op ... public`, the same router and gate as
// a native public op. A regression looks like a private column in an anonymous
// answer, or a private external answering a visitor.

mod anonymous {
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::Request;
    use ordo_conformance::{fixture_space, Fixture, Pg};
    use ordo_host::runtime::{assemble_embedded, door, CustodyChoice, RuntimeConfig};
    use serde_json::Value;
    use tower::util::ServiceExt;

    const PROGRAM: &str = r#"
external catalog (key: cat) from crm.catalog shared {
  public
  id <- "item_id"
  title: text <- "title"
  price: int <- "price"
}

external ledger (key: led) from crm.ledger shared {
  id <- "entry_id"
  label: text public <- "label"
  cost: int <- "cost"
}

external vault (key: vlt) from crm.vault shared {
  id <- "vault_id"
  secret: text <- "secret"
}

op catalogList public =
  catalog[title, price]
  sort title
  take 50

op ledgerLabels public =
  ledger[label]
  take 50

op vaultList =
  vault[secret]
  take 50
"#;

    const SEED: &str = r#"
DROP SCHEMA IF EXISTS crm CASCADE;
CREATE SCHEMA crm;
CREATE TABLE crm.catalog (item_id bigint PRIMARY KEY, title text NOT NULL, price bigint NOT NULL);
INSERT INTO crm.catalog VALUES (1, 'basic', 500), (2, 'pro', 2000);
CREATE TABLE crm.ledger (entry_id bigint PRIMARY KEY, label text NOT NULL, cost bigint NOT NULL);
INSERT INTO crm.ledger VALUES (1, 'rent', 9999);
CREATE TABLE crm.vault (vault_id bigint PRIMARY KEY, secret text NOT NULL);
INSERT INTO crm.vault VALUES (1, 'hunter2');
"#;

    fn free_addr() -> std::net::SocketAddr {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
    }

    async fn boot(stem: &str) -> (ordo_host::runtime::Runtime, axum::Router) {
        let pg = Pg::scoped(stem);
        let f = Fixture::from_source(PROGRAM, SEED).expect("the program compiles");
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
            replica: "external-public-test".into(),
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
        let rt = assemble_embedded(cfg, f).await.expect("boots");
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

    #[tokio::test(flavor = "multi_thread")]
    async fn a_visitor_reads_a_public_external_through_a_public_op() {
        let (_rt, app) = boot("ordo_ext_public").await;

        let (status, text, body) = visit(&app, "/op/catalogList").await;
        assert_eq!(status, 200, "{text}");
        assert_eq!(body["rows"][0]["title"], "basic", "{text}");
        assert_eq!(body["rows"].as_array().unwrap().len(), 2, "{text}");

        // A field mark: the marked field, and never the other.
        let (status, text, body) = visit(&app, "/op/ledgerLabels").await;
        assert_eq!(status, 200, "{text}");
        assert_eq!(body["rows"][0]["label"], "rent", "{text}");
        assert!(!text.contains("cost") && !text.contains("9999"), "{text}");

        // A derived read is never public, like every derived op.
        for uri in [
            "/op/std.catalog.list",
            "/op/std.catalog.get?id=cat_1",
            "/op/std.catalog.count",
            "/op/std.ledger.list",
        ] {
            let (status, text, body) = visit(&app, uri).await;
            assert_eq!(status, 403, "{uri}: {text}");
            assert_eq!(body["error"]["code"], "access_denied", "{uri}: {text}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_visitor_is_refused_a_private_external() {
        let (_rt, app) = boot("ordo_ext_private").await;
        for uri in ["/op/vaultList", "/op/std.vault.list", "/op/std.vault.count"] {
            let (status, text, body) = visit(&app, uri).await;
            assert_eq!(status, 403, "{uri}: {text}");
            assert_eq!(body["error"]["code"], "access_denied", "{uri}: {text}");
            assert!(!text.contains("hunter2"), "{uri}: {text}");
        }
    }
}

/// A native public op gets no cache clock because an unrelated external
/// exists: `reads_external` is what the bound query read, nothing more.
#[test]
fn only_an_op_that_reads_an_external_has_a_cache_clock() {
    let src = format!(
        "{FIXTURE}\n{PROGRAM}\n\
         entity notices {{ body: text public }}\n\
         op noticeList public =\n  notices[body]\n  sort body\n  take 50\n\
         op patronNames =\n  patrons[name]\n  take 50\n"
    );
    let f = Fixture::from_source(&src, SEED).expect("compiles");
    let specs = ordo_host::build_specs(&f);
    let reads = |op: &str| {
        specs
            .iter()
            .find(|s| s.name == op)
            .unwrap_or_else(|| panic!("{op}"))
            .reads_external
    };
    assert!(!reads("noticeList"), "a native op reads no external");
    assert!(reads("patronNames"));
}
