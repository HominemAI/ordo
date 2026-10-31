//! Stage three: the wants expressed in Ordo, judged by the compiler and the
//! seeded database.
//!
//! Stage two established that the wants are reachable — independent authors
//! satisfied them in SQL. This stage asks the question the corpus was built
//! for: can ORDO say the same intents, and when it cannot, is the gap named?
//!
//! The verdict discipline is enforced, not narrated:
//!   - an `ordo` entry MUST be equivalent — a compile error or a row
//!     mismatch fails this suite, and a human decides whether the Ordo or
//!     the compiler is wrong;
//!   - an `inexpressible` entry MUST stay inexpressible — the claim carries
//!     a reason, and nothing here can quietly outgrow it. When the language
//!     gains the feature, this suite fails and forces reclassification.

use std::collections::BTreeMap;

use ordo_conformance::{Fixture, Pg};
use serde_json::Value;

const WANTS: &str = include_str!("../corpus/wants.json");
const ORDO: &str = include_str!("../corpus/ordo.json");

struct Stage3 {
    wants: Vec<Value>,
    cases: BTreeMap<String, Value>,
    fixture: Fixture,
    pg: Pg,
}

fn stage3() -> Stage3 {
    let w: Value = serde_json::from_str(WANTS).expect("wants.json");
    let o: Value = serde_json::from_str(ORDO).expect("ordo.json");

    let fixture = Fixture::load_wide().expect("wide fixture compiles");
    let pg = Pg::named("ordo_wide");

    static ONCE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let f = Fixture::load_wide()?;
        let p = Pg::named("ordo_wide");
        f.reset(&p)
    })
    .clone()
    .expect("wide DDL applies and seeds");

    let cases = o["cases"]
        .as_object()
        .expect("cases")
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    Stage3 {
        wants: w["wants"].as_array().unwrap().clone(),
        cases,
        fixture,
        pg,
    }
}

/// A run_case result (array of objects) projected to the want's column order.
fn rows_in_columns(got: &Value, columns: &[String]) -> Result<Vec<Value>, String> {
    got.as_array()
        .ok_or("not an array")?
        .iter()
        .map(|row| {
            let obj = row.as_object().ok_or("row is not an object")?;
            let vals: Result<Vec<Value>, String> = columns
                .iter()
                .map(|c| {
                    obj.get(c)
                        .cloned()
                        .ok_or_else(|| format!("output has no column `{c}`"))
                })
                .collect();
            Ok(Value::Array(vals?))
        })
        .collect()
}

fn satisfies(got: &Value, expect: &Value) -> Result<(), String> {
    if expect["kind"] == "count" {
        let n = got
            .as_array()
            .and_then(|r| r.first())
            .and_then(|r| r.as_object())
            .and_then(|o| o.values().next())
            .and_then(|v| v.as_i64());
        let want = expect["n"].as_i64();
        return if n == want {
            Ok(())
        } else {
            Err(format!("returned count {n:?}, wanted {want:?}"))
        };
    }
    let columns: Vec<String> = expect["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    let ordered = expect["ordered"].as_bool().unwrap();

    let mut g = rows_in_columns(got, &columns)?;
    let mut w: Vec<Value> = expect["rows"].as_array().cloned().unwrap_or_default();
    if !ordered {
        g.sort_by_key(|r| r.to_string());
        w.sort_by_key(|r| r.to_string());
    }
    if g == w {
        return Ok(());
    }
    // Data-phrased gap, same discipline as stage two.
    let gk: Vec<String> = g.iter().map(|r| r.to_string()).collect();
    let wk: Vec<String> = w.iter().map(|r| r.to_string()).collect();
    let extra: Vec<&Value> = g.iter().filter(|r| !wk.contains(&r.to_string())).collect();
    let missing: Vec<&Value> = w.iter().filter(|r| !gk.contains(&r.to_string())).collect();
    let mut msg = format!("returned {} row(s), wanted {}", g.len(), w.len());
    if !extra.is_empty() {
        msg.push_str(&format!("\n    extra: {extra:?}"));
    }
    if !missing.is_empty() {
        msg.push_str(&format!("\n    missing: {missing:?}"));
    }
    if extra.is_empty() && missing.is_empty() {
        msg.push_str(&format!(
            "\n    same rows, different order\n    returned: {g:?}\n    wanted:   {w:?}"
        ));
    }
    Err(msg)
}

#[test]
fn every_mechanical_want_is_classified() {
    let s = stage3();
    let missing: Vec<&str> = s
        .wants
        .iter()
        .filter(|w| {
            w.get("expect")
                .map(|e| e["kind"] != "relation")
                .unwrap_or(false)
        })
        .map(|w| w["id"].as_str().unwrap())
        .filter(|id| !s.cases.contains_key(*id))
        .collect();
    assert!(
        missing.is_empty(),
        "wants with no stage-three classification: {missing:?}"
    );
}

#[test]
fn ordo_attempts_are_equivalent_and_inexpressibles_stay_inexpressible() {
    let s = stage3();
    let mut failures: Vec<String> = Vec::new();
    let mut equivalent = 0usize;
    let mut inexpressible = Vec::new();

    for w in &s.wants {
        let id = w["id"].as_str().unwrap();
        let Some(expect) = w.get("expect") else {
            continue;
        };
        if expect["kind"] == "relation" {
            continue;
        }
        let Some(case) = s.cases.get(id) else {
            continue;
        };

        if let Some(reason) = case.get("inexpressible") {
            // The claim must carry its reason...
            if reason.as_str().unwrap_or("").trim().is_empty() {
                failures.push(format!("{id}: inexpressible with no reason"));
            }
            inexpressible.push(id.to_string());
            continue;
        }

        let src = case["ordo"].as_str().expect("ordo source");
        match s.fixture.run_case(&s.pg, src) {
            Err(e) => failures.push(format!(
                "{id}: COMPILE/RUN ERROR — my Ordo is wrong or the compiler has a gap:\n  {e}"
            )),
            Ok(got) => match satisfies(&got, expect) {
                Ok(()) => equivalent += 1,
                Err(gap) => failures.push(format!(
                    "{id}: SEMANTIC DIVERGENCE — compiles, returns different rows:\n  {gap}"
                )),
            },
        }
    }

    println!(
        "stage three: {equivalent} equivalent, {} inexpressible {:?}",
        inexpressible.len(),
        inexpressible
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));

    // The inexpressible set is PINNED. A mutation test showed that without
    // this, a false "inexpressible" slides in silently — the runner cannot
    // disprove a negative, so growing this list is a human decision made
    // here, with the reason reviewed in ordo.json. Shrinking it (a want
    // became expressible) is progress and also lands here, deliberately.
    inexpressible.sort();
    assert_eq!(
        inexpressible,
        ["W049"],
        "the inexpressible set changed — reclassify deliberately, in both files"
    );
}

#[test]
fn ruling3_amendment2_witnesses() {
    // The four empty-group witnesses, verbatim from the ruling: on a team
    // with nobody, count = 0, count(cond) = 0 (no phantom from the null-
    // extension row), sum = 0 (not null), avg = null (not 0).
    let s = stage3();
    let got = s
        .fixture
        .run_case(
            &s.pg,
            "teams[team: name]  people[]
group team { n: count, noNick: count(nickname is null), pay: sum salary, meanAge: avg age }",
        )
        .expect("witness query runs");
    let rows = got.as_array().unwrap();
    let platform = rows
        .iter()
        .find(|r| r["team"] == "Platform")
        .expect("the empty team appears at all — that is the whole point");
    assert_eq!(platform["n"], 0, "bare count over the empty group");
    assert_eq!(
        platform["noNick"], 0,
        "count(cond) must not conjure a phantom"
    );
    assert_eq!(
        platform["pay"], 0,
        "sum coalesces to 0 over the empty group"
    );
    assert!(platform["meanAge"].is_null(), "avg over nothing is null");
    // A populated group for contrast: Core = Ada + Bo.
    let core = rows.iter().find(|r| r["team"] == "Core").unwrap();
    assert_eq!(core["n"], 2);
    assert_eq!(core["noNick"], 1, "Ada has no nickname; Bo does");
    assert_eq!(core["pay"], 210_000);
}

#[test]
fn relations_hold_over_the_ordo_results() {
    // The same partition / sum / stability relations as stage two, now over
    // what the ORDO pipelines return — the compiler inherits the semantic
    // obligations, not just the row sets.
    let s = stage3();
    let run = |id: &str| -> Option<Value> {
        let case = s.cases.get(id)?;
        let src = case.get("ordo")?.as_str()?;
        s.fixture.run_case(&s.pg, src).ok()
    };

    let mut failures = Vec::new();
    for w in &s.wants {
        let id = w["id"].as_str().unwrap();
        let Some(expect) = w.get("expect") else {
            continue;
        };
        if expect["kind"] != "relation" {
            continue;
        }
        let op = expect["op"].as_str().unwrap();
        let parts: Vec<&str> = expect["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect();

        // A relation whose parts are not all expressible cannot be tested in
        // Ordo yet; that is a gap, not a pass, so it is printed.
        let results: Vec<Option<Value>> = parts.iter().map(|p| run(p)).collect();
        if results.iter().any(|r| r.is_none()) {
            println!("{id}: skipped — a part is not expressible in Ordo yet");
            continue;
        }
        let results: Vec<Value> = results.into_iter().flatten().collect();

        match op {
            "partition" | "sum_is" => {
                let of = expect
                    .get("of")
                    .or_else(|| expect.get("n"))
                    .and_then(|v| v.as_i64())
                    .unwrap();
                let mut total = 0i64;
                let mut seen: Vec<String> = Vec::new();
                for r in &results {
                    let rows = r.as_array().unwrap();
                    total += rows.len() as i64;
                    seen.extend(rows.iter().map(|x| x.to_string()));
                }
                let mut dedup = seen.clone();
                dedup.sort();
                dedup.dedup();
                if total != of {
                    failures.push(format!(
                        "{id}: parts {} returned {total} rows together, wanted {of}",
                        parts.join(" + ")
                    ));
                }
                if op == "partition" && dedup.len() != seen.len() {
                    failures.push(format!("{id}: the parts overlap"));
                }
            }
            "stable_across_runs" => {
                for p in &parts {
                    let a = run(p).unwrap();
                    let b = run(p).unwrap();
                    if a != b {
                        failures.push(format!("{id}: {p} differed across two runs"));
                    }
                }
            }
            other => failures.push(format!("{id}: unknown relation `{other}`")),
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// The `whole` read is guarded like every other read.
///
/// It was not. `plan_whole` took no caller and inlined no guard, so the
/// one op that returns an entire object graph was the one op with no
/// authorization — and the `members` signpost handed out
/// `principal`/`role`/`status` to anybody who asked. Found by an audit
/// that ran the docs' own examples rather than reading them.
#[test]
fn whole_inlines_the_anchors_read_guard() {
    let f = Fixture::load().expect("fixture");
    let pg = Pg::named("ordo_conf_whole");
    f.reset_once(&pg).expect("seeded");

    // task_1 lives in proj_1. Ana may read it; Dov may not; anonymous may
    // not. The whole-document read must agree with the ordinary read.
    let ana = f
        .run_whole_as(&pg, r#"whole tasks("task_1")"#, Some("mem_1"))
        .expect("Ana's whole read");
    assert_eq!(
        ana["title"], "draft spec",
        "the anchor Ana can read comes back: {ana}"
    );

    for (who, label) in [(Some("mem_4"), "Dov"), (None, "anonymous")] {
        let got = f
            .run_whole_as(&pg, r#"whole tasks("task_1")"#, who)
            .expect("the read runs");
        assert!(
            got.is_null(),
            "{label} must get nothing from a guarded whole read, got: {got}"
        );
    }

    // An UNGUARDED entity is unaffected — the guard is inlined where one
    // exists, not invented where none does.
    let v = f
        .run_whole_as(&pg, r#"whole vendors("vend_1")"#, Some("mem_4"))
        .expect("unguarded whole read");
    assert!(!v.is_null(), "an unguarded anchor still reads: {v}");
}

/// A `tree owned` entity's `whole` used to recurse on the tree self-edge
/// and abort the process with a stack overflow. A legal, spec-blessed
/// shape must not kill the server.
#[test]
fn whole_on_a_tree_owned_entity_terminates() {
    use ordo_schema::build;
    use ordo_syntax::parse_program;

    let src = r#"
entity nodes {
  parent -> nodes?  tree owned
  name: text
}
"#;
    let p = parse_program(src).expect("parses");
    let schema = build(&p).expect("schema builds");
    // The assertion is simply that this returns rather than overflowing:
    // a tree's subtree is a RUNTIME depth, not a schema shape, so it
    // cannot become a nested json_build_object chain at plan time.
    let sql =
        ordo_plan::whole::plan_whole(&schema, "nodes", "'n1'", &ordo_plan::Space::bound("s1"))
            .expect("plans without recursing forever");
    assert!(sql.contains("\"nodes\""), "{sql}");
    assert!(
        !sql.contains("json_agg"),
        "the self-edge must not expand into a nested aggregate:\n{sql}"
    );
}
