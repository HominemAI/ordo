//! Paging under a refinement, proved through the door against Postgres.
//!
//! Ruled 2026-10-05 (`refinement-default` ask 1): conditions compile into the
//! plan, so the page window sits on the refined set. What that has to mean,
//! each a test here:
//!
//! - **Pages come back full**, and walking a refined read two rows at a
//!   time gives exactly the rows one big page gives, in the same order.
//! - **A cursor reused with a changed refinement is harmless.** The cursor
//!   is the last row's sort keys and the envelope has no sort key, so a
//!   new filter starts after the same row in the op's own order. It cannot
//!   widen what the caller sees (`ordo-api/src/page.rs`).
//! - **`take n` stays post-take.** A condition narrows the op's n rows; it
//!   never reaches past the take for others.
//! - **`std.E.count` counts what `std.E.list` lists** under the same
//!   refinement (`std-library.md` §2.1, ruled this round).
//!
//! Needs the conformance cluster. Its own databases, prefixed
//! `ordo_refpage_`.

mod refine_support;

use axum::http::StatusCode;
use refine_support::{all_pages, call, door, ids, path, sorts};
use serde_json::{json, Value};

fn refinements() -> Vec<Value> {
    vec![
        json!({"where": []}),
        json!({"where": [{"field": "n", "op": "ne", "value": 5}]}),
        json!({"where": [{"field": "t", "op": "gt", "value": "Zed"}]}),
        json!({"where": [{"field": "m", "op": "in", "value": ["2.5", "29.99", null]}]}),
        json!({"where": [{"field": "b", "op": "is_null"}], "pick": ["id", "n"]}),
        json!({"where": [{"field": "n", "op": "ge", "value": 0}, {"field": "t", "op": "ne", "value": "apple"}]}),
    ]
}

fn ops() -> Vec<String> {
    let mut ops = vec!["probeRows".to_string()];
    ops.extend((0..sorts().len()).map(|i| format!("probeSort{i}")));
    ops
}

/// Two rows a page through a refined set is the refined set: same rows,
/// same order, every page but the last full.
#[tokio::test]
async fn small_pages_through_a_refined_set_are_the_whole_refined_set() {
    let (small, _f) = door("ordo_refpage_small", 2).await;
    let (big, _g) = door("ordo_refpage_big", 50).await;
    for op in ops() {
        for refine in refinements() {
            let whole = all_pages(&big, &op, &refine).await.expect("served");
            let paged = all_pages(&small, &op, &refine).await.expect("served");
            assert_eq!(ids(&paged), ids(&whole), "{op} {refine}");

            // Full pages: the window is on the refined set, not before it.
            let mut page: Option<String> = None;
            let mut sizes = Vec::new();
            loop {
                let (status, body) = call(
                    &small,
                    "GET",
                    &path(&op, "", Some(&refine), page.as_deref()),
                    None,
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{body}");
                sizes.push(body["rows"].as_array().unwrap().len());
                match body["next"].as_str() {
                    Some(n) => page = Some(n.to_string()),
                    None => break,
                }
            }
            let (last, rest) = sizes.split_last().unwrap();
            assert!(rest.iter().all(|s| *s == 2), "{op} {refine}: {sizes:?}");
            assert!(*last <= 2);
        }
    }
}

/// A cursor from one refinement, presented with another: accepted, and
/// harmless. The new filter starts after the same row in the op's order,
/// which is what a parameter swap already does (`page.rs`).
#[tokio::test]
async fn a_cursor_reused_with_a_changed_refinement_starts_after_the_same_row() {
    let (small, _f) = door("ordo_refpage_cursor", 2).await;
    let op = "probeSort0";
    let first = json!({"where": [{"field": "n", "op": "ne", "value": 5}]});
    let second = json!({"where": [{"field": "t", "op": "ne", "value": "apple"}]});

    let (status, body) = call(&small, "GET", &path(op, "", Some(&first), None), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after = body["rows"][1]["id"].as_str().unwrap().to_string();
    let cursor = body["next"].as_str().expect("more pages").to_string();

    // The op's own order, unrefined, places the cursor row.
    let order = ids(&all_pages(&small, op, &json!({"where": []})).await.unwrap());
    let at = order.iter().position(|i| *i == after).unwrap();
    let second_set = ids(&all_pages(&small, op, &second).await.unwrap());
    let want: Vec<String> = second_set
        .into_iter()
        .filter(|i| order.iter().position(|o| o == i).unwrap() > at)
        .collect();

    let mut got = Vec::new();
    let mut page = Some(cursor);
    while let Some(p) = page {
        let (status, body) =
            call(&small, "GET", &path(op, "", Some(&second), Some(&p)), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        got.extend(ids(body["rows"].as_array().unwrap()));
        page = body["next"].as_str().map(String::from);
    }
    assert_eq!(got, want);
}

/// `take n` settles the op's rows, and a refinement filters those rows. It
/// never reaches past the take for other rows that would match.
#[tokio::test]
async fn take_stays_post_take() {
    let (state, _f) = door("ordo_refpage_take", 2).await;
    let top = all_pages(&state, "probeTop", &json!({"where": []}))
        .await
        .unwrap();
    assert_eq!(top.len(), 4);
    let refine = json!({"where": [{"field": "n", "op": "lt", "value": 12}]});
    let want: Vec<String> = top
        .iter()
        .filter(|r| r["n"].as_i64().is_some_and(|n| n < 12))
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    let got = all_pages(&state, "probeTop", &refine).await.unwrap();
    assert_eq!(ids(&got), want);
    assert!(
        got.len() < 4,
        "the take's other rows are not reached for: {got:?}"
    );
}

/// `std.probes.count` and `std.probes.list` under one refinement describe
/// one set (`std-library.md` §2.1): the count's conditions name the list's
/// fields and apply before the count.
#[tokio::test]
async fn count_counts_what_list_lists() {
    let (state, _f) = door("ordo_refpage_count", 2).await;
    for refine in refinements() {
        let refine = json!({"where": refine["where"]});
        let listed = all_pages(&state, "std.probes.list", &refine).await.unwrap();
        let (status, body) = call(
            &state,
            "GET",
            &path("std.probes.count", "", Some(&refine), None),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{refine}: {body}");
        assert_eq!(
            body["rows"][0]["n"].as_u64().unwrap() as usize,
            listed.len(),
            "{refine}"
        );
    }
    // `in []` counts nothing.
    let none = json!({"where": [{"field": "n", "op": "in", "value": []}]});
    let (status, body) = call(
        &state,
        "GET",
        &path("std.probes.count", "", Some(&none), None),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["n"], 0);
    // The count's own `n` is not a field a condition names.
    let on_n = json!({"where": [{"field": "n", "op": "gt", "value": 0}]});
    let (status, _) = call(
        &state,
        "GET",
        &path("std.probes.count", "", Some(&on_n), None),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "`n` here is the list's `n`, the probes column"
    );
}
