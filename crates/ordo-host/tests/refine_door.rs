//! The door's `$refine`, applied, against Postgres.
//!
//! Ruled 2026-10-05 (`refinement-default` asks 1, 3 and 6): conditions on
//! open and blind-index fields compile into the plan, a sealed condition
//! with no blind index is refused at the door naming the field, and a
//! refinement's size is a setting. Until this suite the executor took the
//! refinement and dropped it, and served the op's unfiltered rows.
//!
//! **Three evaluators, one answer.** The shared parity table
//! (`ordo-conformance/fixtures/refine-parity.json`) is read by the device's
//! half (`clients/ordo-sdk/test/refine-parity.test.ts`), by written
//! composition (`ordo-conformance/tests/refine_parity.rs`), and here, where
//! every case goes through the door as a `$refine` on a registered op and
//! must answer the recorded ids in order.
//!
//! It lives in `ordo-host` and not `ordo-conformance`, where the plan put
//! it, because the door is this crate's: `ordo-conformance` depends on
//! neither `ordo-api` nor `ordo-host`.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`). Its
//! own databases, prefixed `ordo_refine_`.

mod refine_support;

use axum::http::StatusCode;
use refine_support::{all_pages, call, door, ids, op_for_sort, path, sorts, table};
use serde_json::{json, Value};

/// The table's sort keys, as the op that carries them.
fn sort_of(case: &Value) -> Vec<(String, bool)> {
    case["sort"]
        .as_array()
        .expect("sort")
        .iter()
        .map(|k| {
            (
                k["field"].as_str().expect("field").to_string(),
                k["direction"] == "desc",
            )
        })
        .collect()
}

/// Every case of the shared table, through the door. The envelope has no
/// sort key, so a case's sort is the op's: one op per distinct sort.
#[tokio::test]
async fn every_parity_case_answers_through_the_door() {
    let (state, _f) = door("ordo_refine_parity", 50).await;
    assert!(!sorts().is_empty());
    let t = table();
    let cases = t["cases"].as_array().expect("cases");
    let mut failures = Vec::new();
    for case in cases {
        let refine = json!({ "where": case["where"] });
        let op = op_for_sort(&sort_of(case));
        let want: Vec<String> = case["expect"]
            .as_array()
            .expect("expect")
            .iter()
            .map(|v| v.as_str().expect("id").to_string())
            .collect();
        match all_pages(&state, &op, &refine).await {
            Ok(rows) => {
                let got = ids(&rows);
                if got != want {
                    failures.push(format!(
                        "{} via {op}\n  door:  {got:?}\n  table: {want:?}",
                        case["name"]
                    ));
                }
            }
            Err(e) => failures.push(format!("{} via {op}\n  refused: {e}", case["name"])),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ at the door:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

async fn rows(state: &ApiState, op: &str, refine: Value) -> Vec<Value> {
    all_pages(state, op, &refine)
        .await
        .unwrap_or_else(|e| panic!("{op} {refine}: {e}"))
}

use ordo_api::ApiState;

fn cond(field: &str, op: &str, value: Value) -> Value {
    json!({"where": [{"field": field, "op": op, "value": value}]})
}

fn labels(rows: &[Value]) -> Vec<&str> {
    rows.iter()
        .map(|r| r["label"].as_str().unwrap_or("<none>"))
        .collect()
}

/// The new cases of `plan-refinement.md` section 6, over types the parity
/// table does not seed: `is_null` on each type, a datetime range, money at
/// 0 places, `-0.00` against `0.00`, `in` over decimals.
#[tokio::test]
async fn the_types_the_table_does_not_reach() {
    let (state, _f) = door("ordo_refine_types", 50).await;

    for field in ["d", "at", "units", "m", "est", "ok"] {
        let got = rows(
            &state,
            "extraRows",
            json!({"where": [{"field": field, "op": "is_null"}]}),
        )
        .await;
        assert_eq!(labels(&got), ["b"], "is_null on `{field}`");
    }

    let range = json!({"where": [
        {"field": "at", "op": "ge", "value": "2026-10-05T08:00:00Z"},
        {"field": "at", "op": "lt", "value": "2026-10-05T12:00:00Z"},
    ]});
    assert_eq!(labels(&rows(&state, "extraRows", range).await), ["a"]);
    let dates = cond("d", "gt", json!("2026-03-01"));
    assert_eq!(labels(&rows(&state, "extraRows", dates).await), ["c"]);

    // money(0): units units, a negative one, and an integer value.
    assert_eq!(
        labels(&rows(&state, "extraRows", cond("units", "lt", json!(0))).await),
        ["c"]
    );
    assert_eq!(
        labels(&rows(&state, "extraRows", cond("units", "eq", json!("5"))).await),
        ["a"]
    );

    // `-0.00` is `0.00`: the parameter path normalizes its zero, and this one
    // must agree.
    for zero in ["-0.00", "0.00", "0", "-0"] {
        assert_eq!(
            labels(&rows(&state, "extraRows", cond("m", "eq", json!(zero))).await),
            ["a"],
            "m eq {zero}"
        );
    }
    let decimals = cond("m", "in", json!(["12.5", "0", null]));
    assert_eq!(
        labels(&rows(&state, "extraRows", decimals).await),
        ["a", "b", "c"]
    );

    // A duration takes the integer the op returns for it, in its unit.
    assert_eq!(
        labels(&rows(&state, "extraRows", cond("est", "ge", json!(8))).await),
        ["a"]
    );
    assert_eq!(
        labels(&rows(&state, "extraRows", cond("ok", "eq", json!(false))).await),
        ["c"]
    );

    // `in []` is a chain of no equalities: it matches nothing, as on the
    // device, and is not a SQL error.
    assert!(rows(&state, "extraRows", cond("label", "in", json!([])))
        .await
        .is_empty());
}

/// `pick` is the projection. The anchor's `$id` rides every row that has
/// one, whether or not the pick names it, because a row's identity is not
/// a presentation choice (`ui-gen.md` §1).
#[tokio::test]
async fn pick_is_the_projection_and_the_anchor_id_rides() {
    let (state, _f) = door("ordo_refine_pick", 50).await;
    let keys = |row: &Value| -> Vec<String> {
        let mut k: Vec<String> = row.as_object().expect("a row").keys().cloned().collect();
        k.sort();
        k
    };

    // The op projects `id`: picking it keeps it, and no `$id` is attached.
    let got = rows(&state, "probeRows", json!({"pick": ["id", "t"]})).await;
    assert_eq!(got.len(), 12);
    assert_eq!(keys(&got[0]), ["id", "t"]);

    // Dropping `id` from the pick: the anchor id rides as `$id`.
    let got = rows(&state, "probeRows", json!({"pick": ["t"]})).await;
    assert_eq!(keys(&got[0]), ["$id", "t"]);

    // An op that never projected its id carries `$id`; a pick keeps it.
    let got = rows(&state, "probeNoId", json!({"pick": ["n"]})).await;
    assert_eq!(keys(&got[0]), ["$id", "n"]);
    let got = rows(&state, "probeNoId", json!({"pick": ["$id"]})).await;
    assert_eq!(keys(&got[0]), ["$id"]);

    // A condition and a pick together, on a field the pick drops.
    let got = rows(
        &state,
        "probeRows",
        json!({"where": [{"field": "n", "op": "eq", "value": 5}], "pick": ["id"]}),
    )
    .await;
    assert_eq!(ids(&got), ["r01", "r05", "r12"]);
    assert_eq!(keys(&got[0]), ["id"]);
}

/// A sealed field with a blind index answers `eq` and `in` in the
/// database. Picking the sealed field away keeps the plan in SQL, so no
/// payload is opened to answer it.
#[tokio::test]
async fn a_sealed_indexed_condition_rides_the_blind_index() {
    let (state, _f) = door("ordo_refine_blind", 50).await;
    for (label, code) in [("one", "K-1"), ("two", "K-2"), ("three", "K-1")] {
        let (status, body) = call(
            &state,
            "POST",
            "/op/addToken",
            Some(json!({"label": label, "code": code})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let got = rows(
        &state,
        "tokenList",
        json!({"where": [{"field": "code", "op": "eq", "value": "K-1"}], "pick": ["label"]}),
    )
    .await;
    assert_eq!(labels(&got), ["one", "three"]);
    let got = rows(
        &state,
        "tokenList",
        json!({"where": [{"field": "code", "op": "in", "value": ["K-2", "K-9"]}], "pick": ["label"]}),
    )
    .await;
    assert_eq!(labels(&got), ["two"]);
}

/// 25as (`sealed-refine-500`): an op settled by a `take` that returns its
/// sealed indexed field. The refinement wraps the op, the plan opens the
/// payloads in the host, and the equality still rides the blind index.
/// Every other operator is a typed refusal naming the field, never a 500.
#[tokio::test]
async fn a_sealed_indexed_condition_on_a_take_that_returns_the_field_is_served() {
    let (state, _f) = door("ordo_refine_blindtake", 50).await;
    for (label, code) in [("one", "K-1"), ("two", "K-2"), ("three", "K-1")] {
        let (status, body) = call(
            &state,
            "POST",
            "/op/addToken",
            Some(json!({"label": label, "code": code})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let codes = |rows: &[Value]| -> Vec<String> {
        rows.iter()
            .map(|r| r["code"].as_str().unwrap_or("<none>").to_string())
            .collect()
    };
    let got = rows(&state, "tokenTop", cond("code", "eq", json!("K-1"))).await;
    assert_eq!(labels(&got), ["one", "three"]);
    assert_eq!(codes(&got), ["K-1", "K-1"], "plaintext");
    let got = rows(
        &state,
        "tokenTop",
        cond("code", "in", json!(["K-2", "K-9"])),
    )
    .await;
    assert_eq!(labels(&got), ["two"]);
    assert_eq!(codes(&got), ["K-2"]);

    for (rop, value) in [
        ("in", json!([])),
        ("ne", json!("K-1")),
        ("lt", json!("K-1")),
        ("le", json!("K-1")),
        ("gt", json!("K-1")),
        ("ge", json!("K-1")),
    ] {
        let (status, body) = call(
            &state,
            "GET",
            &path("tokenTop", "", Some(&cond("code", rop, value)), None),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{rop}: {body}");
        assert_eq!(body["error"]["code"], "invalid_refinement", "{rop}");
        assert_eq!(body["error"]["param"], "code", "{rop}");
    }
    let (status, body) = call(
        &state,
        "GET",
        &path(
            "tokenTop",
            "",
            Some(&json!({"where": [{"field": "code", "op": "is_null"}]})),
            None,
        ),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
}

/// Ruled 2026-10-05 (`refinement-default` ask 3): a condition on a sealed
/// field with no blind index is refused AT THE DOOR, naming the field and
/// the fix, until host-side filtering is built. Never a 500, and never the
/// executor's message that named neither.
#[tokio::test]
async fn a_sealed_unindexed_condition_is_refused_at_the_door() {
    let (state, _f) = door("ordo_refine_sealed", 50).await;
    let (status, body) = call(
        &state,
        "POST",
        "/op/addJotting",
        Some(json!({"label": "a", "body": "secret"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let refuse = |op: &'static str| {
        let state = state.clone();
        async move {
            call(
                &state,
                "GET",
                &path(op, "", Some(&cond("body", "eq", json!("secret"))), None),
                None,
            )
            .await
        }
    };

    // Bounded by its own `take 5`: the cost gate passes it, and the venue
    // refuses it, naming the field and the fix.
    let (status, body) = refuse("jotTop").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "sealed_unsupported");
    assert_eq!(body["error"]["param"], "body");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("sealed indexed refinable"),
        "the refusal names the fix: {body}"
    );

    // A paged listing: the condition would open every row in the space, so
    // the cost gate answers first, with its own class. Also the field.
    let (status, body) = refuse("jotList").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "refinement_unbounded");
    assert_eq!(body["error"]["param"], "body");
}

/// A value the door would once have passed to the binder, where it was a
/// 500, is a 400 naming the field.
#[tokio::test]
async fn a_mistyped_value_is_a_400_and_never_a_500() {
    let (state, _f) = door("ordo_refine_mistyped", 50).await;
    for (field, value) in [
        ("n", json!("five")),
        ("t", json!(5)),
        ("m", json!(2.5)),
        ("r", json!("0,5")),
        ("b", json!("yes")),
    ] {
        let (status, body) = call(
            &state,
            "GET",
            &path(
                "probeRows",
                "",
                Some(&cond(field, "eq", value.clone())),
                None,
            ),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field} = {value}: {body}");
        assert_eq!(body["error"]["code"], "invalid_refinement");
        assert_eq!(body["error"]["param"], field);
    }
}

/// The call is built as a tree, so a parameter's value is never spelled as
/// source. The text path stripped `"` from a text parameter, so a value
/// with a quote in it matched a different string.
#[tokio::test]
async fn a_text_parameter_keeps_its_quotes() {
    let (state, _f) = door("ordo_refine_quotes", 50).await;
    let phrase = |p: &'static str| {
        let state = state.clone();
        async move {
            let (status, body) = call(
                &state,
                "GET",
                &path(
                    "phraseIs",
                    &format!("p={}", refine_support::encode(p)),
                    None,
                    None,
                ),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            body["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["p"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    // The stripping path answered `say hi` for this one.
    assert_eq!(phrase(r#"say "hi""#).await, [r#"say "hi""#]);
    assert_eq!(phrase("say hi").await, ["say hi"]);
}

fn row_keys(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|r| r["$id"].as_str().unwrap_or("<no $id>").to_string())
        .collect()
}

/// `$id` is refinable (C4e, 2026-09-17: *"`$id` is the attached name, and it
/// is refinable ... Filtering a synced list by row id is a reasonable thing
/// to permit"*). The op's body cannot name it, so the executor applies its
/// conditions to the bound query, on the anchor's id column.
#[tokio::test]
async fn the_attached_id_takes_a_condition() {
    let (state, _f) = door("ordo_refine_row_key", 5).await;
    let all = row_keys(&rows(&state, "probeNoId", json!({})).await);
    assert_eq!(all.len(), 12, "{all:?}");
    let (a, b) = (all[3].clone(), all[7].clone());

    let got = rows(&state, "probeNoId", cond("$id", "eq", json!(a))).await;
    assert_eq!(row_keys(&got), std::slice::from_ref(&a));

    let got = rows(
        &state,
        "probeNoId",
        cond("$id", "in", json!([b.clone(), a.clone(), null])),
    )
    .await;
    assert_eq!(
        row_keys(&got),
        [a.clone(), b.clone()],
        "in keeps the op's order"
    );

    let got = rows(&state, "probeNoId", cond("$id", "ne", json!(a))).await;
    let rest: Vec<String> = all.iter().filter(|i| **i != a).cloned().collect();
    assert_eq!(row_keys(&got), rest, "ne drops one row, across pages of 5");

    assert!(rows(&state, "probeNoId", cond("$id", "in", json!([])))
        .await
        .is_empty());
    assert!(rows(
        &state,
        "probeNoId",
        json!({"where": [{"field": "$id", "op": "is_null"}]})
    )
    .await
    .is_empty());

    // Ids order as text, so the order operators compare them as the device
    // does.
    let mut sorted = all.clone();
    sorted.sort();
    let pivot = sorted[5].clone();
    let mut got = row_keys(&rows(&state, "probeNoId", cond("$id", "lt", json!(pivot))).await);
    got.sort();
    assert_eq!(got, sorted[..5]);

    // Beside a condition on an open field, with a pick of `$id` alone.
    let got = rows(
        &state,
        "probeNoId",
        json!({"where": [
            {"field": "$id", "op": "in", "value": [a.clone(), b.clone()]},
            {"field": "n", "op": "gt", "value": 0},
        ], "pick": ["$id"]}),
    )
    .await;
    let positive = row_keys(&rows(&state, "probeNoId", cond("n", "gt", json!(0))).await);
    let want: Vec<String> = [a, b]
        .into_iter()
        .filter(|i| positive.contains(i))
        .collect();
    assert_eq!(row_keys(&got), want);
    assert!(got
        .iter()
        .all(|r| r.as_object().is_some_and(|o| o.len() == 1)));
}

/// A `take` settles the op's rows, and a condition on `$id` applies after
/// it, as every condition does: an id outside the top 4 matches nothing.
#[tokio::test]
async fn a_condition_on_the_attached_id_applies_after_the_take() {
    let (state, _f) = door("ordo_refine_row_key_take", 50).await;
    let top = row_keys(&rows(&state, "probeTopNoId", json!({})).await);
    assert_eq!(top.len(), 4);
    let all = row_keys(&rows(&state, "probeNoId", json!({})).await);
    let outside = all
        .iter()
        .find(|i| !top.contains(i))
        .expect("12 rows, 4 on top")
        .clone();
    let got = rows(
        &state,
        "probeTopNoId",
        cond("$id", "in", json!([outside, top[2].clone()])),
    )
    .await;
    assert_eq!(row_keys(&got), [top[2].clone()]);
}

/// An op that projects its own id attaches no `$id`, so a condition on
/// `$id` is a field the op does not return: refused at the door.
#[tokio::test]
async fn no_attached_id_no_condition_on_it() {
    let (state, _f) = door("ordo_refine_row_key_door", 50).await;
    let (status, body) = call(
        &state,
        "GET",
        &path(
            "probeRows",
            "",
            Some(&cond("$id", "eq", json!("r01"))),
            None,
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_refinement", "{body}");
}

/// The descriptor's `refine.limits` and the door's settings ship the same
/// defaults: a client checks a refinement against the first before the
/// wire, and the door refuses past the second.
#[test]
fn the_descriptor_and_the_door_ship_one_set_of_limits() {
    let doc = ordo_plan::interface::RefineLimits::default();
    let door = ordo_api::RefineLimits::default();
    assert_eq!(doc.conditions, door.max_conditions);
    assert_eq!(doc.in_values, door.max_in_values);
    assert_eq!((doc.conditions, doc.in_values), (16, 200));
}

/// 25av: a refinement the door passed and the binder refuses is the
/// caller's refusal. The door's own checks catch these first, so the test
/// hands the executor a refinement the door would have stopped: `ne` on a
/// sealed indexed field. It answers its class and names the field, not 500.
#[tokio::test]
async fn a_refinement_that_fails_to_bind_is_refused_with_its_class_not_500() {
    use ordo_api::{BoundParams, ErrorCode, RefineCond, RefineOp, Refinement};
    let (state, _f) = door("ordo_refine_bindfail", 50).await;
    let op = state.registry.current().get("tokenTop").expect("op");
    let bad = Refinement {
        where_: vec![RefineCond {
            field: "code".into(),
            op: RefineOp::Ne,
            value: json!("K-1"),
        }],
        pick: None,
    };
    let bound = BoundParams(
        json!({"$caller": "member:mem_1"})
            .as_object()
            .expect("map")
            .clone(),
    );
    let err = state
        .executor
        .execute(
            &op,
            ordo_host::DEFAULT_SPACE,
            bound.clone(),
            Some(bad.clone()),
            None,
            None,
        )
        .await
        .expect_err("the binder refuses it");
    assert_ne!(err.code, ErrorCode::Internal, "{err}");
    assert!(
        matches!(
            err.code,
            ErrorCode::InvalidRefinement | ErrorCode::SealedUnsupported
        ),
        "{err}"
    );
    assert!(err.message.contains("`code`"), "names the field: {err}");

    // The mapping: a bind code with a condition is the caller's; the same
    // text with no condition to blame, and any other text, stays 500.
    let text = "bind: [sealed_unsupported] rule: `code` is sealed at 1:2";
    let e = ordo_host::refine_bind_refusal(text, Some(&bad)).expect("mapped");
    assert_eq!(e.code, ErrorCode::SealedUnsupported);
    let e = ordo_host::refine_bind_refusal("bind: [no_match_path] x at 1:1", Some(&bad))
        .expect("mapped");
    assert_eq!(e.code, ErrorCode::InvalidRefinement);
    assert!(e.message.contains("`code`"), "{}", e.message);
    assert!(ordo_host::refine_bind_refusal(text, None).is_none());
    assert!(ordo_host::refine_bind_refusal("pool: gone", Some(&bad)).is_none());
}
