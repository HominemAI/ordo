//! 27ao (`serve-sealed-reads`) at the real door: a read that returns a
//! sealed field is served, and each page it opens is one key release,
//! recorded as a `KeyReleased` security event exactly as every other
//! release is, through the metered provider.
//!
//! The executor's own suite (`crates/ordo-exec/tests/sealed_reads.rs`)
//! covers the shapes: a single row, a join, a guard, anonymous, and the
//! refusal of `unbounded-host`. This one covers what only the door shows:
//! the request's correlation on the release, and the page as the unit.
//!
//! Needs the conformance cluster. Its own databases, prefixed
//! `ordo_sealread_`.

mod refine_support;

use std::sync::Arc;

use axum::http::StatusCode;
use ordo_secevent::{EventKind, Recorder};
use refine_support::{call, call_as, door_keyed, path};
use serde_json::{json, Value};

/// The read releases the stream holds since the last drain, as their
/// correlations.
fn read_releases(stream: &Recorder) -> Vec<String> {
    stream
        .drain_all(ordo_commandlog::Timestamp(0))
        .into_iter()
        .filter(|e| {
            e.kind
                == EventKind::KeyReleased {
                    purpose: "read".into(),
                }
        })
        .map(|e| e.correlations[0].0.clone())
        .collect()
}

async fn metered_door(db: &str, page_size: i64) -> (ordo_api::ApiState, Arc<Recorder>) {
    let stream = Arc::new(Recorder::default());
    let s = stream.clone();
    let (state, _f) = door_keyed(db, page_size, move |inner| {
        Arc::new(
            ordo_host::metrics::MeteredKeys::new(
                inner,
                Arc::new(ordo_host::metrics::Metrics::new()),
            )
            .with_stream(s),
        )
    })
    .await;
    (state, stream)
}

/// `std.vault.list` over seven sealed rows at three a page: three pages,
/// plaintext on every row, and exactly one read release per page, each
/// under its own request's correlation.
#[tokio::test]
async fn a_sealed_list_is_served_page_by_page_with_one_release_per_page() {
    let (state, stream) = metered_door("ordo_sealread_pages", 3).await;
    for i in 1..=7 {
        let (status, body) = call(
            &state,
            "POST",
            "/op/addSecret",
            Some(json!({"label": format!("l{i}"), "secret": format!("S-{i}")})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let _ = read_releases(&stream);

    let mut secrets: Vec<String> = Vec::new();
    let mut correlations: Vec<String> = Vec::new();
    let mut page: Option<String> = None;
    let mut pages = 0;
    loop {
        let (status, body) = call(
            &state,
            "GET",
            &path("std.vault.list", "", None, page.as_deref()),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        pages += 1;
        let rows = body["rows"].as_array().unwrap();
        assert!(rows.len() <= 3, "{body}");
        for r in rows {
            let label = r["label"].as_str().unwrap();
            let secret = r["secret"].as_str().unwrap();
            assert_eq!(&label[1..], &secret[2..], "the row's own plaintext: {r}");
            secrets.push(secret.to_string());
        }
        let released = read_releases(&stream);
        assert_eq!(
            released.len(),
            1,
            "one release for page {pages}: {released:?}"
        );
        correlations.extend(released);
        match body["next"].as_str() {
            Some(n) => page = Some(n.to_string()),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    secrets.sort();
    assert_eq!(
        secrets,
        (1..=7).map(|i| format!("S-{i}")).collect::<Vec<_>>()
    );
    let mut distinct = correlations.clone();
    distinct.dedup();
    assert_eq!(distinct.len(), 3, "each request its own: {correlations:?}");
    assert!(correlations.iter().all(|c| c.starts_with("cor_")));
}

/// The audit's program for reported fix 25ap, as an author writes it.
const LOOKUP: &str = "entity customers {\n  name:  text\n  email: text sealed indexed\n}\n\n\
op byEmail($e: text) = customers(email == $e)[name, email] take 1\n";

/// Reported fix 25ap: a declared op that looks up by blind index and returns
/// the sealed field applies the way `ordo apply` applies it, and the door
/// serves the same shape (`lookupOf`, `lookups(code == $c)[label, code]
/// take 1`) with the opened value under one release.
///
/// The op classes `bounded-host` (its payload is opened on the host) and its
/// compare still rides the digest. The artifact used to withhold the
/// statement only for class `blind-index`, so it planned this op, and the
/// apply refused the whole program with `unresolved_blind_index`. A
/// regression fails the apply here before the door is reached.
#[tokio::test]
async fn a_declared_blind_index_lookup_applies_and_returns_the_opened_value() {
    let target = ordo_conformance::Pg::named("ordo_sealread_apply");
    target
        .exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("empty target");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        size: 3,
        db: target.db.clone(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("target pool");
    let report = ordo_host::apply_op::run(
        &pool,
        &ordo_exec::route::SinglePool::new(pool.clone()),
        LOOKUP,
        &Default::default(),
        "sealed-reads-test",
        "ordo apply",
        true,
    )
    .await
    .unwrap_or_else(|e| panic!("the apply: {e}"));
    assert!(
        matches!(
            report.activation,
            ordo_host::apply_op::Activation::Switched { .. }
        ),
        "{report:?}"
    );

    let program = ordo_host::schema_apply::Program::compile(LOOKUP).expect("compiles");
    let op = program.doc["ops"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["op"] == "byEmail")
        .expect("byEmail is in the artifact")
        .clone();
    assert_eq!(op["error"], Value::Null, "{op}");
    assert_eq!(op["cost"]["class"], "bounded-host", "{op}");
    assert_eq!(
        op["plan"]["sql"],
        Value::Null,
        "the digest is withheld: {op}"
    );

    let (state, stream) = metered_door("ordo_sealread_lookup", 50).await;
    for (label, secret) in [("a", "S-1"), ("b", "S-2"), ("c", "S-3")] {
        let (status, body) = call(
            &state,
            "POST",
            "/op/addLookup",
            Some(json!({"label": label, "code": secret})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let _ = read_releases(&stream);

    let (status, body) = call(&state, "GET", &path("lookupOf", "c=S-2", None, None), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{body}");
    assert_eq!(rows[0]["label"], "b");
    assert_eq!(rows[0]["code"], "S-2", "the opened value: {body}");
    assert_eq!(read_releases(&stream).len(), 1);

    let (status, body) = call(&state, "GET", &path("lookupOf", "c=S-9", None, None), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["rows"].as_array().unwrap().is_empty(), "{body}");
}

/// A refined read that returns the sealed value opens only the rows the
/// blind index found, under one release. A `pick` that leaves the sealed
/// field out opens nothing and releases nothing. Anonymous is refused at
/// the door and releases nothing.
#[tokio::test]
async fn refinement_pick_and_anonymous_release_only_what_they_open() {
    let (state, stream) = metered_door("ordo_sealread_refined", 50).await;
    for (label, secret) in [("a", "S-1"), ("b", "S-2"), ("c", "S-1")] {
        let (status, body) = call(
            &state,
            "POST",
            "/op/addSecret",
            Some(json!({"label": label, "secret": secret})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let _ = read_releases(&stream);

    let eq = json!({"where": [{"field": "secret", "op": "eq", "value": "S-2"}]});
    let (status, body) = call(
        &state,
        "GET",
        &path("std.vault.list", "", Some(&eq), None),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows: &Vec<Value> = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{body}");
    assert_eq!(rows[0]["label"], "b");
    assert_eq!(rows[0]["secret"], "S-2");
    assert_eq!(read_releases(&stream).len(), 1);

    let pick = json!({"pick": ["label"]});
    let (status, body) = call(
        &state,
        "GET",
        &path("std.vault.list", "", Some(&pick), None),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"].as_array().unwrap().len(), 3);
    assert!(
        read_releases(&stream).is_empty(),
        "nothing sealed was opened"
    );

    let (status, body) = call_as(
        &state,
        None,
        "GET",
        &path("std.vault.list", "", None, None),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(
        read_releases(&stream).is_empty(),
        "anonymous opened nothing"
    );
}
