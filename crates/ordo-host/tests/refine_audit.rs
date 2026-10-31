//! The refinement audit (R8, `plan-refinement.md` section 11): what a
//! hostile or careless caller can do to the door with `$refine`, and what
//! a refinement can never do.
//!
//! - **No 500s.** Every registered read, every field it has, every operator,
//!   against values of the wrong shape, the wrong type, hostile size and
//!   hostile nesting. Each answer is a success or a typed refusal.
//! - **A refinement never widens.** A row the guard hides stays hidden under
//!   any condition, `$id` included; a `pick` returns no field outside the
//!   op's outputs; anonymous callers are refused at the door.
//! - **The sealed rules at the real door.** A count refuses a sealed
//!   condition (rule 3); a list with a blind index serves `eq` and `in` and
//!   refuses the ordering operators. The list returns its sealed field as
//!   plaintext, refined or not (27ao).
//! - **The size limits at the boundary**, through the real executor.
//!
//! Needs the conformance cluster. Its own databases, prefixed
//! `ordo_refaudit_`.

mod refine_support;

use axum::http::StatusCode;
use ordo_api::{ApiState, OpKind, ParamTy};
use refine_support::{call, call_as, call_full, door, door_with_pool, path};
use serde_json::{json, Value};

const MEM_1: Option<&str> = Some("member:mem_1");
const MEM_2: Option<&str> = Some("member:mem_2");
const MEM_4: Option<&str> = Some("member:mem_4");

/// A value every parameter type binds, so the request reaches the executor.
fn dummy(ty: &ParamTy) -> &'static str {
    match ty {
        ParamTy::Int => "1",
        ParamTy::Money { .. } | ParamTy::Decimal { .. } => "1",
        ParamTy::Text { .. } => "x",
        ParamTy::Date => "2026-01-01",
        ParamTy::Datetime => "2026-01-01T00:00:00Z",
        ParamTy::Bool => "true",
        ParamTy::Id => "r_01",
        ParamTy::Duration => "1",
        ParamTy::Uri => "https://example.test/x",
        // A JSON string is a document like any other.
        ParamTy::Json => "x",
    }
}

fn hostile_scalars() -> Vec<Value> {
    vec![
        Value::Null,
        json!(true),
        json!(1),
        json!(-1),
        json!(2.5),
        json!(1e300),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(""),
        json!("x"),
        json!("5"),
        json!("-0"),
        json!("+5"),
        json!("1."),
        json!(".5"),
        json!("1e5"),
        json!("99999999999999999999999999"),
        json!("2026-01-01"),
        json!("2026-13-45"),
        json!("2026-02-31"),
        json!("2026-02-29"),
        json!("2024-02-29"),
        json!("0000-01-01"),
        json!("2026-02-30T00:00:00Z"),
        json!("2026-01-01T23:59:60Z"),
        json!("2.505"),
        json!("99999999999999999999"),
        json!("9".repeat(40)),
        json!("-9".to_string() + &"9".repeat(40)),
        json!(format!("{}.5", "9".repeat(40))),
        json!("2026-10-05T09:00:00Z"),
        json!("2026-10-05T09:00:00+02:00"),
        json!("\u{0}"),
        json!("e\u{301}"),
        json!("'; DROP TABLE probes; --"),
        json!("x".repeat(20_000)),
        json!("a\u{0}b"),
        json!([]),
        json!([1]),
        json!({}),
        json!({"a": {"b": [[[1]]]}}),
    ]
}

fn hostile_lists() -> Vec<Value> {
    vec![
        json!([]),
        json!([null]),
        json!([null, null]),
        json!([1, "a", true, null, 2.5]),
        json!([2.5]),
        json!(["x"]),
        json!([[1]]),
        json!([{}]),
        json!(["2026-01-01", "x"]),
        json!((0..201).collect::<Vec<i64>>()),
        json!("not a list"),
        json!(1),
    ]
}

/// A typed refusal or a success: never a 500, and every refusal names a
/// class. `Err` says what was wrong, so a sweep can report every case.
fn typed_or_why(status: StatusCode, body: &Value, what: &str) -> Result<(), String> {
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        return Err(format!("a 500 for {what}: {body}"));
    }
    if status != StatusCode::OK {
        // A full connection pool is a documented, retryable 503 (reported
        // fix 25z). Under full-suite load the sweep's 5,000-odd requests
        // can meet one, and it says nothing about the refinement.
        let busy = status == StatusCode::SERVICE_UNAVAILABLE
            && body["error"]["code"].as_str() == Some("server_busy");
        if !(status.is_client_error() || busy) {
            return Err(format!("{what}: {status} {body}"));
        }
        if !body["error"]["code"]
            .as_str()
            .is_some_and(|c| !c.is_empty())
        {
            return Err(format!("{what}: a refusal with no class: {body}"));
        }
    }
    Ok(())
}

fn typed(status: StatusCode, body: &Value, what: &str) {
    if let Err(why) = typed_or_why(status, body, what) {
        panic!("{why}");
    }
}

/// Every row's keys are outputs of the op, or the attached `$id`.
fn within_outputs(op: &ordo_api::OpSpec, body: &Value, what: &str) {
    let outputs: Vec<&str> = op
        .output_fields
        .iter()
        .filter(|f| f.output)
        .map(|f| f.name.as_str())
        .collect();
    for row in body["rows"].as_array().into_iter().flatten() {
        for key in row.as_object().into_iter().flat_map(|o| o.keys()) {
            assert!(
                key == "$id" || outputs.contains(&key.as_str()),
                "{what}: row carries `{key}`, outside {outputs:?}: {row}"
            );
        }
    }
}

/// Every registered read of the suite's program, with the query string that
/// binds its parameters.
fn reads(state: &ApiState, f: &ordo_conformance::Fixture) -> Vec<(ordo_api::OpSpec, String)> {
    let registry = state.registry.current();
    let mut names: Vec<&String> = f.registry.names().collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|n| registry.get(n))
        .filter(|op| op.kind == OpKind::Read)
        .map(|op| {
            let params: Vec<String> = op
                .params
                .iter()
                .map(|p| format!("{}={}", p.name, refine_support::encode(dummy(&p.ty))))
                .collect();
            (op, params.join("&"))
        })
        .collect()
}

/// The ops whose fields cover every scalar type, and the attached `$id`:
/// these take the whole value matrix. Every other read takes a shorter one,
/// since what differs there is the op's shape, not its field types.
const TYPED: [&str; 3] = ["probeRows", "probeNoId", "extraRows"];

/// The whole matrix for `eq` and `lt`; the other ordering operators share
/// their path and take a sample.
const SAMPLE: [&str; 2] = ["eq", "lt"];

/// Every operator on every field of every read, against values of the
/// wrong shape and type, hostile size and hostile nesting. The requests
/// are built first and sent 16 at a time; each answer is a success, within
/// the op's outputs, or a typed refusal.
#[tokio::test(flavor = "multi_thread")]
async fn every_operator_on_every_field_against_hostile_values_is_never_a_500() {
    use futures::StreamExt;
    let (state, f) = door("ordo_refaudit_sweep", 50).await;
    let ops = reads(&state, &f);
    assert!(ops.len() > 40, "{} reads", ops.len());

    let few = || {
        vec![
            Value::Null,
            json!(1),
            json!("x"),
            json!(2.5),
            json!("5"),
            json!("2026-02-31"),
            json!("a\u{0}b"),
            json!("9".repeat(40)),
            json!([]),
            json!({}),
        ]
    };
    // (op, params, refinement, what)
    let mut requests: Vec<(ordo_api::OpSpec, String, Value, String)> = Vec::new();
    for (op, params) in &ops {
        let whole = TYPED.contains(&op.name.as_str());
        // The derived std ops share one derivation: two entities' worth
        // (one open, one `std sealed`) stand for them all.
        if op.name.starts_with("std.")
            && !(op.name.starts_with("std.probes.") || op.name.starts_with("std.vault."))
        {
            continue;
        }
        for field in &op.output_fields {
            let mut probes: Vec<(&str, Value)> = Vec::new();
            for rop in ["eq", "ne", "lt", "le", "gt", "ge"] {
                let values = if whole && SAMPLE.contains(&rop) {
                    hostile_scalars()
                } else if whole || rop == "eq" {
                    few()
                } else {
                    continue;
                };
                for v in values {
                    probes.push((rop, v));
                }
            }
            let lists = if whole {
                hostile_lists()
            } else {
                vec![json!([null]), json!([1, "a", 2.5, null])]
            };
            for v in lists {
                probes.push(("in", v));
            }
            let nulls = if whole {
                vec![Value::Null, json!(1), json!([])]
            } else {
                vec![Value::Null]
            };
            for v in nulls {
                probes.push(("is_null", v));
            }
            for (rop, value) in probes {
                let refine = json!({"where": [{"field": field.name, "op": rop, "value": value}]});
                let what = format!("{} {} {rop} {}", op.name, field.name, short(&refine));
                requests.push((op.clone(), params.clone(), refine, what));
            }
            // A pick of this one field, and of it twice.
            for pick in [json!([field.name]), json!([field.name, field.name])] {
                let refine = json!({"pick": pick});
                let what = format!("{} pick {}", op.name, short(&refine));
                requests.push((op.clone(), params.clone(), refine, what));
            }
        }
    }
    let total = requests.len();

    let answers: Vec<(ordo_api::OpSpec, String, StatusCode, Value)> =
        futures::stream::iter(requests)
            .map(|(op, params, refine, what)| {
                let state = state.clone();
                async move {
                    let (status, body) = call(
                        &state,
                        "GET",
                        &path(&op.name, &params, Some(&refine), None),
                        None,
                    )
                    .await;
                    (op, what, status, body)
                }
            })
            .buffer_unordered(16)
            .collect()
            .await;

    let mut served = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for (op, what, status, body) in &answers {
        if let Err(why) = typed_or_why(*status, body, what) {
            failures.push(why);
        }
        if *status == StatusCode::OK {
            served += 1;
            within_outputs(op, body, what);
        }
    }
    assert!(served > 100, "{served} of {total} requests served");
    assert!(
        failures.is_empty(),
        "{} of {total} requests were not typed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 120 {
        format!("{}…", &s[..120])
    } else {
        s
    }
}

/// Envelopes that are not the envelope: wrong shapes, unknown operators,
/// unknown keys, fields that are not fields, picks of nothing and of the
/// same thing twice, and junk where JSON should be.
#[tokio::test]
async fn malformed_envelopes_are_typed_refusals() {
    let (state, _f) = door("ordo_refaudit_shape", 50).await;
    let envelopes = [
        json!([]),
        json!("x"),
        json!(1),
        json!(null),
        json!({"where": {}}),
        json!({"where": null}),
        json!({"where": [1]}),
        json!({"where": [{"field": 1, "op": "eq", "value": 1}]}),
        json!({"where": [{"op": "eq", "value": 1}]}),
        json!({"where": [{"field": "n"}]}),
        json!({"where": [{"field": "n", "op": "like", "value": 1}]}),
        json!({"where": [{"field": "n", "op": "EQ", "value": 1}]}),
        json!({"where": [{"field": "n", "op": "eq", "value": 1, "extra": 1}]}),
        json!({"where": [{"field": "", "op": "eq", "value": 1}]}),
        json!({"where": [{"field": "$space", "op": "eq", "value": "x"}]}),
        json!({"where": [{"field": "$caller", "op": "eq", "value": "mem_2"}]}),
        json!({"where": [{"field": "$id", "op": "eq", "value": "r01"}]}),
        json!({"where": [{"field": "x".repeat(20_000), "op": "eq", "value": 1}]}),
        json!({"where": [{"field": "n", "op": "eq", "value": {"a": {"b": [[[1]]]}}}]}),
        json!({"pick": "n"}),
        json!({"pick": null}),
        json!({"pick": [1]}),
        json!({"pick": []}),
        json!({"pick": ["n", "n"]}),
        json!({"pick": ["n", "id", "n"]}),
        json!({"pick": ["$id"]}),
        json!({"pick": ["$id", "$id"]}),
        json!({"pick": ["colour"]}),
        json!({"zzz": 1}),
        json!({"where": [], "pick": []}),
    ];
    for op in [
        "probeRows",
        "probeNoId",
        "probeTop",
        "std.probes.count",
        "std.probes.list",
    ] {
        for envelope in &envelopes {
            let what = format!("{op} {}", short(envelope));
            let (status, body) =
                call(&state, "GET", &path(op, "", Some(envelope), None), None).await;
            typed(status, &body, &what);
        }
    }
    // Not JSON at all.
    for raw in ["%7B", "x", "%7B%22where%22%3A%5B", ""] {
        let (status, body) =
            call(&state, "GET", &format!("/op/probeRows?$refine={raw}"), None).await;
        typed(status, &body, raw);
        assert_eq!(status, StatusCode::BAD_REQUEST, "{raw}: {body}");
    }
    // A refinement on a write, and on a write with a body.
    let (status, body) = call(
        &state,
        "POST",
        "/op/addToken",
        Some(json!({"label": "a", "code": "K", "$refine": {"pick": ["label"]}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_refinement");
}

fn row_keys(body: &Value) -> Vec<String> {
    body["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| r["$id"].as_str().unwrap_or("<no $id>").to_string())
        .collect()
}

async fn rows_as(state: &ApiState, token: Option<&str>, op: &str, refine: Value) -> Vec<Value> {
    let mut rows = Vec::new();
    let mut page: Option<String> = None;
    loop {
        let (status, body) = call_as(
            state,
            token,
            "GET",
            &path(op, "", Some(&refine), page.as_deref()),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{op} {refine} as {token:?}: {body}");
        rows.extend(body["rows"].as_array().cloned().unwrap_or_default());
        match body["next"].as_str() {
            Some(n) => page = Some(n.to_string()),
            None => return rows,
        }
    }
}

/// A refinement never widens: under any condition, a caller sees a subset
/// of what the unrefined op shows them, and a condition on `$id` naming a
/// row the guard hides finds nothing. Three members with different
/// memberships, then no member at all.
#[tokio::test]
async fn a_refinement_never_reaches_past_the_guard() {
    let (state, _f) = door("ordo_refaudit_guard", 2).await;
    let all_ids = row_keys(&json!({"rows": rows_as(&state, MEM_1, "allTasks", json!({})).await}));
    assert!(all_ids.len() >= 5, "{all_ids:?}");

    let open =
        json!({"where": [{"field": "status", "op": "eq", "value": "open"}], "pick": ["title"]});
    let everything = json!({"where": [{"field": "$id", "op": "in", "value": all_ids}]});
    let mut seen_by: Vec<(Option<&str>, Vec<String>)> = Vec::new();
    for member in [MEM_1, MEM_2, MEM_4] {
        let mine = rows_as(&state, member, "taskBoard", json!({})).await;
        let mine_ids = row_keys(&json!({"rows": mine}));
        // A condition keeps a subset, and exactly the rows that meet it.
        let refined = rows_as(&state, member, "taskBoard", open.clone()).await;
        let want: Vec<String> = mine
            .iter()
            .filter(|r| r["status"] == "open")
            .map(|r| r["$id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            row_keys(&json!({"rows": refined.clone()})),
            want,
            "{member:?}"
        );
        for row in &refined {
            let keys: Vec<&String> = row.as_object().unwrap().keys().collect();
            assert_eq!(keys, ["$id", "title"], "{member:?}: {row}");
        }
        // Naming every task by id gives back exactly the caller's own rows.
        let named = rows_as(&state, member, "taskBoard", everything.clone()).await;
        assert_eq!(row_keys(&json!({"rows": named})), mine_ids, "{member:?}");
        // Naming one hidden row finds nothing, and is not a refusal.
        for hidden in all_ids.iter().filter(|i| !mine_ids.contains(i)) {
            let one = json!({"where": [{"field": "$id", "op": "eq", "value": hidden}]});
            let got = rows_as(&state, member, "taskBoard", one).await;
            assert!(got.is_empty(), "{member:?} reached {hidden}: {got:?}");
            let not_it = json!({"where": [{"field": "$id", "op": "ne", "value": hidden}]});
            let got = rows_as(&state, member, "taskBoard", not_it).await;
            assert_eq!(
                row_keys(&json!({"rows": got})),
                mine_ids,
                "{member:?} ne {hidden}"
            );
        }
        seen_by.push((member, mine_ids));
    }
    // The three see different things, so the checks above were not vacuous.
    assert!(seen_by[0].1 != seen_by[1].1, "{seen_by:?}");
    assert!(
        seen_by[2].1.is_empty(),
        "mem_4 is on no project: {seen_by:?}"
    );

    // No bearer, a bearer that is no member, and a member of no space.
    for token in [None, Some("member:nobody"), Some("x")] {
        let (status, body) = call_as(
            &state,
            token,
            "GET",
            &path("taskBoard", "", Some(&open), None),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{token:?}: {body}");
        assert_eq!(body["error"]["code"], "access_denied", "{token:?}: {body}");
    }
}

/// The sealed rules at the real door, on an entity whose std ops see its
/// sealed field (`std sealed`): the count refuses a sealed condition
/// outright (rule 3), the list serves equality through the blind index and
/// refuses the rest, and a `pick` of the open field opens no payload.
#[tokio::test]
async fn a_count_refuses_a_sealed_condition_and_a_list_serves_equality_only() {
    let (state, _f) = door("ordo_refaudit_sealed", 50).await;
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
    let secret =
        |rop: &str, v: Value| json!({"where": [{"field": "secret", "op": rop, "value": v}]});

    let (status, body) = call(
        &state,
        "GET",
        &path(
            "std.vault.count",
            "",
            Some(&secret("eq", json!("S-1"))),
            None,
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_refinement");
    assert_eq!(body["error"]["param"], "secret");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("aggregate-only"),
        "{body}"
    );
    // The count still counts under an open condition.
    let (status, body) = call(
        &state,
        "GET",
        &path(
            "std.vault.count",
            "",
            Some(&json!({"where": [{"field": "label", "op": "ne", "value": "b"}]})),
            None,
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["n"], 2);

    let labels = |rows: &[Value]| -> Vec<String> {
        rows.iter()
            .map(|r| r["label"].as_str().unwrap().to_string())
            .collect()
    };
    let secrets = |rows: &[Value]| -> Vec<String> {
        rows.iter()
            .map(|r| r["secret"].as_str().unwrap().to_string())
            .collect()
    };
    // The list projects the sealed field, so it plans as `bounded-host`: the
    // page's rows are fetched as stored and their envelopes open in the
    // host (27ao, `serve-sealed-reads`). The caller gets plaintext.
    let (status, body) = call(&state, "GET", &path("std.vault.list", "", None, None), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(labels(rows), ["c", "b", "a"], "newest first");
    assert_eq!(secrets(rows), ["S-1", "S-2", "S-1"], "plaintext: {body}");
    // A refined read that returns the sealed value: the equality rides the
    // blind index, and the rows it keeps open as the unrefined list's do.
    let got = rows_as(
        &state,
        MEM_1,
        "std.vault.list",
        json!({"where": [{"field": "secret", "op": "eq", "value": "S-1"}]}),
    )
    .await;
    assert_eq!(labels(&got), ["c", "a"]);
    assert_eq!(secrets(&got), ["S-1", "S-1"]);
    // Picking the sealed field away keeps the plan in SQL, and the sealed
    // equality still rides the blind index.
    let got = rows_as(
        &state,
        MEM_1,
        "std.vault.list",
        json!({"where": [{"field": "secret", "op": "eq", "value": "S-1"}], "pick": ["label"]}),
    )
    .await;
    assert_eq!(labels(&got), ["c", "a"], "newest first");
    let got = rows_as(
        &state,
        MEM_1,
        "std.vault.list",
        json!({"where": [{"field": "secret", "op": "in", "value": ["S-2", "S-9"]}], "pick": ["label"]}),
    )
    .await;
    assert_eq!(labels(&got), ["b"]);
    assert!(got.iter().all(|r| r.get("secret").is_none()), "{got:?}");
    for rop in ["ne", "lt", "le", "gt", "ge"] {
        let (status, body) = call(
            &state,
            "GET",
            &path("std.vault.list", "", Some(&secret(rop, json!("S-1"))), None),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{rop}: {body}");
        assert_eq!(body["error"]["code"], "invalid_refinement", "{rop}");
        assert_eq!(body["error"]["param"], "secret", "{rop}");
    }
    let (status, body) = call(
        &state,
        "GET",
        &path(
            "std.vault.list",
            "",
            Some(&json!({"where": [{"field": "secret", "op": "is_null"}]})),
            None,
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = call(
        &state,
        "GET",
        &path("std.vault.list", "", Some(&secret("in", json!([]))), None),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("at least one value"));
}

/// The size limits at the boundary, through the real executor: the limit
/// is served, one past it is refused naming the setting and its default.
#[tokio::test]
async fn the_size_limits_hold_at_the_boundary_through_the_real_door() {
    let (state, _f) = door("ordo_refaudit_limits", 50).await;
    let conditions = |n: usize| -> Value {
        json!({"where": (0..n).map(|i| json!({"field": "n", "op": "ne", "value": 100 + i as i64})).collect::<Vec<_>>()})
    };
    let got = rows_as(&state, MEM_1, "probeRows", conditions(16)).await;
    assert_eq!(got.len(), 12, "16 conditions, none of which drops a row");
    let (status, body) = call(
        &state,
        "GET",
        &path("probeRows", "", Some(&conditions(17)), None),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("ORDO_REFINE_MAX_CONDITIONS") && message.contains("(default 16)"),
        "{message}"
    );

    let in_values = |n: i64| -> Value {
        json!({"where": [{"field": "n", "op": "in", "value": (0..n).collect::<Vec<i64>>()}]})
    };
    let got = rows_as(&state, MEM_1, "probeRows", in_values(200)).await;
    assert!(!got.is_empty(), "200 values is the limit, not past it");
    let (status, body) = call(
        &state,
        "GET",
        &path("probeRows", "", Some(&in_values(201)), None),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["param"], "n");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("ORDO_REFINE_MAX_IN_VALUES") && message.contains("(default 200)"),
        "{message}"
    );
}

/// Reported fix 25z. A pool of one connection, held by one transaction: the
/// next read and the next write each wait out the bounded acquire and are
/// answered `server_busy`, a 503 with `Retry-After`. Once the connection is
/// free, the identical requests land.
#[tokio::test(flavor = "multi_thread")]
async fn a_full_pool_answers_server_busy_and_the_retry_lands() {
    let (state, _f, pool) = door_with_pool(
        "ordo_refaudit_busy",
        50,
        ordo_exec::PoolConfig {
            size: 1,
            acquire_timeout: std::time::Duration::from_millis(200),
            ..ordo_exec::PoolConfig::from_env()
        },
    )
    .await;
    let read = path("probeRows", "", None, None);
    let write = json!({"label": "busy", "code": "K-1"});

    // Cold: the space's first request reads its continuity on the pool,
    // so the resolver meets the full pool before the executor does.
    let held = pool.begin().await.expect("the only connection");
    let (status, body) = call_as(&state, MEM_1, "GET", &read, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "server_busy", "{body}");
    held.rollback().await.expect("release");
    // A busy moment does not hold the tenant closed.
    let (status, body) = call_as(&state, MEM_1, "GET", &read, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Warm: the executor's own checkout is the one refused.
    let held = pool.begin().await.expect("the only connection");
    let (status, headers, body) = call_full(&state, MEM_1, "GET", &read, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "server_busy", "{body}");
    assert_eq!(body["error"]["op"], "probeRows", "{body}");
    assert_eq!(headers.get("retry-after").unwrap(), "1");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("Nothing ran") && !message.contains("server_busy"),
        "{message}"
    );
    let (status, body) = call_as(&state, MEM_1, "POST", "/op/addToken", Some(write.clone())).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "server_busy", "{body}");
    held.rollback().await.expect("release");

    let (status, body) = call_as(&state, MEM_1, "GET", &read, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call_as(&state, MEM_1, "POST", "/op/addToken", Some(write)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "nothing landed the first time: {body}"
    );
}
