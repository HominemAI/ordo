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

op customerByEmail($e: text) =
  patrons(email == $e)[id, name]
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
        "/op/customerByEmail?e=cy%40example.com",
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
