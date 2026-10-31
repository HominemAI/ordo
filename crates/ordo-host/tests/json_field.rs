//! 28b: a `json` field at the real door, against Postgres.
//!
//! A document goes in as JSON, is stored as `jsonb` (or sealed), and comes
//! back as the same JSON. The door bounds its size and depth. A write
//! converges by `rev` as any field does. A condition on one is refused.
//!
//! A regression looks like a document coming back as a string holding
//! JSON, a sealed document coming back as text, an over-limit document
//! reaching the database, or a stale `rev` overwriting a newer document.
//!
//! Needs the conformance cluster. Its own databases, prefixed
//! `ordo_json_`.

mod refine_support;

use axum::http::StatusCode;
use refine_support::{call, door, path};
use serde_json::{json, Value};

async fn add(state: &ordo_api::ApiState, doc: Value) -> String {
    let (status, body) = call(
        state,
        "POST",
        "/op/addDocument",
        Some(json!({"title": "t", "doc": doc})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    created(&body)
}

fn created(body: &Value) -> String {
    body["rows"][0]["created"]
        .as_str()
        .unwrap_or_else(|| panic!("a created id: {body}"))
        .to_string()
}

async fn read(state: &ordo_api::ApiState, id: &str) -> Value {
    let (status, body) = call(
        state,
        "GET",
        &path("documentOf", &format!("d={id}"), None, None),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["rows"][0].clone()
}

#[tokio::test]
async fn a_json_field_round_trips_an_object_an_array_a_string_and_a_number_exactly() {
    let (state, _) = door("ordo_json_roundtrip", 50).await;
    let docs = [
        json!({"name": "Ada", "tags": ["a", "b"], "n": 1.5, "nested": {"ok": true, "none": null}}),
        json!([1, "two", {"three": 3}, [], {}]),
        json!("a string with \"quotes\", a \\ backslash,\na newline and é"),
        json!(42),
        json!(-0.125),
        json!(true),
    ];
    for doc in docs {
        let id = add(&state, doc.clone()).await;
        let row = read(&state, &id).await;
        assert_eq!(row["doc"], doc, "the document comes back as JSON: {row}");
    }
}

#[tokio::test]
async fn an_omitted_json_field_takes_its_empty_default() {
    let (state, _) = door("ordo_json_default", 50).await;
    let (status, body) = call(
        &state,
        "POST",
        "/op/std.documents.create",
        Some(json!({"title": "t"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(&state, "GET", "/op/std.documents.list", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["doc"], json!({}), "{body}");
}

#[tokio::test]
async fn a_document_over_the_size_limit_is_refused_as_bound_violation() {
    let (state, _) = door("ordo_json_limit", 50).await;
    let max = ordo_api::json::limits().max_bytes;
    let big = json!({"text": "x".repeat(max)});
    let (status, body) = call(
        &state,
        "POST",
        "/op/addDocument",
        Some(json!({"title": "t", "doc": big})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "bound_violation", "{body}");
    assert!(
        body.to_string().contains("ORDO_JSON_MAX_BYTES"),
        "the refusal names the setting: {body}"
    );

    let mut deep = json!(1);
    for _ in 0..=ordo_api::json::limits().max_depth {
        deep = json!([deep]);
    }
    let (status, body) = call(
        &state,
        "POST",
        "/op/addDocument",
        Some(json!({"title": "t", "doc": deep})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "bound_violation", "{body}");

    let (status, body) = call(
        &state,
        "POST",
        "/op/addDocument",
        Some(json!({"title": "t", "doc": {"a": "nul\u{0}"}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_param", "{body}");

    let (status, body) = call(&state, "GET", "/op/std.documents.list", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["rows"].as_array().unwrap().is_empty(),
        "nothing was written: {body}"
    );
}

#[tokio::test]
async fn a_sealed_json_field_round_trips() {
    let (state, _) = door("ordo_json_sealed", 50).await;
    let id = add(&state, json!({})).await;
    let secret = json!({"diagnosis": "flu", "codes": [1, 2]});
    let (status, body) = call(
        &state,
        "POST",
        "/op/sealDocument",
        Some(json!({"d": id, "secret": secret})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(read(&state, &id).await["secret"], secret);

    // The column holds ciphertext, never the document.
    let pg = ordo_conformance::Pg::named("ordo_json_sealed");
    let raw = pg
        .query(&format!(
            "SELECT encode(\"secret\", 'escape') FROM \"documents\" WHERE id = '{id}'"
        ))
        .expect("raw")[0][0]
        .clone()
        .unwrap_or_default();
    assert!(!raw.contains("diagnosis"), "sealed at rest: {raw}");
}

#[tokio::test]
async fn an_edit_to_a_json_field_conflicts_on_rev_like_any_field() {
    let (state, _) = door("ordo_json_rev", 50).await;
    let id = add(&state, json!({"v": 1})).await;
    let (status, body) = call(
        &state,
        "POST",
        "/op/saveDocument",
        Some(json!({"d": id, "rev": 1, "doc": {"v": 2}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &state,
        "POST",
        "/op/saveDocument",
        Some(json!({"d": id, "rev": 1, "doc": {"v": 3}})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "cas_miss", "{body}");
    assert_eq!(read(&state, &id).await["doc"], json!({"v": 2}));
}

#[tokio::test]
async fn a_refinement_condition_on_a_json_field_is_refused() {
    let (state, _) = door("ordo_json_refine", 50).await;
    let cond = json!({"where": [{"field": "doc", "op": "eq", "value": {}}]});
    let (status, body) = call(
        &state,
        "GET",
        &path("std.documents.list", "", Some(&cond), None),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_refinement", "{body}");
    assert!(body.to_string().contains("json"), "{body}");
}

/// A condition on a path into the document, with its parameter bound by the
/// door as a real call binds it (filtering ruled 2026-10-08).
#[tokio::test]
async fn a_condition_on_a_path_into_the_document_filters_with_bound_parameters() {
    let (state, _) = door("ordo_json_filter", 50).await;
    add(&state, json!({"theme": "dark", "n": 3})).await;
    add(&state, json!({"theme": "light", "n": "many"})).await;
    add(&state, json!({"n": 10.5})).await;
    let titles = |body: &Value| -> usize { body["rows"].as_array().map_or(0, Vec::len) };
    let (status, body) = call(
        &state,
        "GET",
        &path("documentsWithTheme", "t=dark", None, None),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(titles(&body), 1, "{body}");
    // `"many"` is not a number: it does not match, and nothing errors.
    let (status, body) = call(
        &state,
        "GET",
        &path("documentsOver", "n=2", None, None),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(titles(&body), 2, "{body}");
}
