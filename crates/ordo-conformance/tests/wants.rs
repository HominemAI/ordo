//! Guards on the frozen wants file.
//!
//! The corpus protocol is: state what we want to know, freeze it, then write a
//! query. The freezing is the whole point — an expectation authored after the
//! query, or edited once a result came back, is not an expectation, it is a
//! transcript. These tests enforce the parts of that discipline a machine can
//! check.
//!
//! What a machine cannot check is whether I wrote a want honestly. That is why
//! the wants carry prose intent as well as a value: a reviewer can read the
//! intent and decide whether the expectation follows from the seed, without
//! reading a single query.

use std::collections::HashSet;

use serde_json::Value;

const WANTS: &str = include_str!("../corpus/wants.json");

fn wants() -> Value {
    serde_json::from_str(WANTS).expect("wants.json is valid JSON")
}

#[test]
fn wants_file_contains_no_query() {
    // Not a keyword scan: "where" and "order" are ordinary English and appear
    // in intent prose. This looks for the shape of a statement instead.
    let lower = WANTS.to_lowercase();
    let sql_shaped = lower
        .match_indices("select")
        .any(|(i, _)| lower[i..(i + 300).min(lower.len())].contains(" from "));
    assert!(!sql_shaped, "a SELECT…FROM appears in the wants file");

    let v = wants();
    for w in v["wants"].as_array().unwrap() {
        for banned in ["sql", "ordo", "query"] {
            assert!(
                w.get(banned).is_none(),
                "{} carries a `{banned}` field — queries live outside this file",
                w["id"]
            );
        }
    }
}

#[test]
fn every_want_is_complete_and_uniquely_identified() {
    let v = wants();
    let ws = v["wants"].as_array().unwrap();
    let mut seen = HashSet::new();

    for w in ws {
        let id = w["id"].as_str().expect("id is a string");
        assert!(seen.insert(id), "duplicate want id {id}");
        assert!(
            !w["intent"].as_str().unwrap_or("").trim().is_empty(),
            "{id} has no intent — an expectation with no stated purpose cannot be reviewed"
        );
        assert!(
            !w["probes"].as_array().map(|a| a.is_empty()).unwrap_or(true),
            "{id} declares no probes, so nothing records what it covers"
        );
        // The two layers are additive: mechanical, judged, or both — but a
        // want with neither states nothing that can ever be settled.
        let has_expect = w.get("expect").is_some();
        let has_judge = w.get("judge").is_some();
        assert!(
            has_expect || has_judge,
            "{id} carries neither an `expect` nor a `judge`, so nothing can ever satisfy it"
        );

        if has_expect {
            let kind = w["expect"]["kind"].as_str().unwrap_or("");
            assert!(
                matches!(kind, "rows" | "count" | "relation"),
                "{id} has expectation kind `{kind}`, which code cannot check"
            );
            // An ordered expectation is a claim about ordering; an unordered
            // one must not smuggle one in by listing rows suggestively.
            if kind == "rows" {
                assert!(
                    w["expect"]["ordered"].is_boolean(),
                    "{id} does not say whether row order is part of the want"
                );
            }
        }

        if has_judge {
            let qs = w["judge"].as_array().expect("judge is a list of questions");
            assert!(!qs.is_empty(), "{id} has an empty judge list");
            for q in qs {
                assert!(
                    !q["asks"].as_str().unwrap_or("").trim().is_empty(),
                    "{id} has a judged item with no question"
                );
                // A verdict without a rationale is an assertion of authority.
                // If one has been recorded, the reasoning must be there too.
                if q.get("verdict").is_some() {
                    assert!(
                        !q["because"].as_str().unwrap_or("").trim().is_empty(),
                        "{id} records a verdict with no `because` — a judged pass \
                         is only worth anything with its reasoning attached"
                    );
                }
            }
        }
    }
}

#[test]
fn relations_reference_wants_that_exist() {
    let v = wants();
    let ws = v["wants"].as_array().unwrap();
    let ids: HashSet<&str> = ws.iter().filter_map(|w| w["id"].as_str()).collect();

    for w in ws {
        if w["expect"]["kind"] != "relation" {
            continue;
        }
        for part in w["expect"]["parts"].as_array().expect("relation has parts") {
            let p = part.as_str().unwrap();
            assert!(ids.contains(p), "{} references unknown want {p}", w["id"]);
        }
    }
}

#[test]
fn coverage_is_visible() {
    // Printed rather than asserted: this is a ledger, and the number that
    // matters is which dimensions have no want at all.
    let v = wants();
    let ws = v["wants"].as_array().unwrap();
    let mut dims: Vec<String> = ws
        .iter()
        .flat_map(|w| w["probes"].as_array().unwrap())
        .map(|p| p.as_str().unwrap().to_string())
        .collect();
    dims.sort();
    dims.dedup();

    // Counted separately and never summed. A mechanical pass is a comparison;
    // a judged pass is an opinion with a rationale. Adding them would produce
    // a single confidence number that overstates the weaker half.
    let mechanical = ws.iter().filter(|w| w.get("expect").is_some()).count();
    let judged = ws.iter().filter(|w| w.get("judge").is_some()).count();
    let questions: usize = ws
        .iter()
        .filter_map(|w| w.get("judge").and_then(|j| j.as_array()))
        .map(|a| a.len())
        .sum();

    println!("frozen wants: {}", ws.len());
    println!("  mechanically checkable: {mechanical}");
    println!("  judged: {judged}  ({questions} questions)");
    println!("probe dimensions: {}", dims.len());
    for d in &dims {
        println!("  {d}");
    }
    println!(
        "retired: {}",
        v["retired"].as_array().map(|a| a.len()).unwrap_or(0)
    );

    assert!(ws.len() >= 40, "the first frozen batch is 40 wants");
    assert!(
        judged > 0,
        "a corpus with no judged wants only tests what code can compare"
    );
}

#[test]
fn every_mechanical_want_is_blind_validated() {
    // The 11-of-40 lesson, made structural: a want the author alone can
    // satisfy is a memo, not a specification. A mechanical want counts only
    // once an isolated author — who never saw the expected values or anyone
    // else's queries — satisfied it from its wording. New wants fail here
    // until they earn that, so the corpus cannot re-import the problem as
    // it grows.
    let v = wants();
    let ledger: Value =
        serde_json::from_str(include_str!("../corpus/validation.json")).expect("validation.json");
    let validated = ledger["validated"].as_object().unwrap();

    let mut unvalidated = Vec::new();
    for w in v["wants"].as_array().unwrap() {
        let Some(expect) = w.get("expect") else {
            continue;
        };
        if expect["kind"] == "relation" {
            continue; // computed from validated parts
        }
        let id = w["id"].as_str().unwrap();
        let ok = validated
            .get(id)
            .and_then(|e| e.as_array())
            .map(|e| !e.is_empty())
            .unwrap_or(false);
        if !ok {
            unvalidated.push(id);
        }
    }
    assert!(
        unvalidated.is_empty(),
        "wants with no blind validation — run an isolated author over the \
         redacted wording before these count: {unvalidated:?}"
    );
}
