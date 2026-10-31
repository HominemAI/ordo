//! Stage two: do the hand-written queries satisfy the frozen wants?
//!
//! The wants were written and frozen before any query existed. Postgres runs
//! the query; code compares the result to the want. Nothing in this file may
//! change a want — a mismatch means the **query** is wrong, and that is the
//! whole point of the ordering.
//!
//! Failure output is deliberately phrased in terms of the data — which rows
//! were extra, which were missing — and never in terms of the query. A retry
//! should be able to re-derive the query from the want plus the gap, without
//! being anchored on the attempt that failed.

use std::collections::BTreeMap;

use ordo_conformance::{Fixture, Pg};
use serde_json::Value;

const WANTS: &str = include_str!("../corpus/wants.json");
const SQL: &str = include_str!("../corpus/sql.json");

struct Corpus {
    wants: Vec<Value>,
    sql: BTreeMap<String, String>,
    fixture: Fixture,
    pg: Pg,
}

fn corpus() -> Corpus {
    let w: Value = serde_json::from_str(WANTS).expect("wants.json");
    let s: Value = serde_json::from_str(SQL).expect("sql.json");

    let fixture = Fixture::load_wide().expect("the wide fixture compiles");
    let pg = Pg {
        db: s["database"].as_str().expect("database").to_string(),
        ..Pg::default()
    };

    // The wide corpus owns its own database, so it seeds independently of the
    // market fixture and the two cannot race.
    static ONCE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    let seeded = ONCE
        .get_or_init(|| {
            let f = Fixture::load_wide()?;
            let p = Pg::named("ordo_wide");
            f.reset(&p)
        })
        .clone();
    seeded.expect("wide DDL applies and seeds");

    let sql = s["queries"]
        .as_object()
        .expect("queries")
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect();

    Corpus {
        wants: w["wants"].as_array().unwrap().clone(),
        sql,
        fixture,
        pg,
    }
}

impl Corpus {
    fn result(&self, id: &str) -> Result<Value, String> {
        let sql = self
            .sql
            .get(id)
            .ok_or_else(|| format!("{id} has no query"))?;
        self.fixture.run_sql(&self.pg, sql)
    }
}

/// Sort rows so an unordered comparison does not depend on the server's
/// choice, while an ordered one still does.
fn normalise(rows: &Value, ordered: bool) -> Vec<Value> {
    let mut v: Vec<Value> = rows.as_array().cloned().unwrap_or_default();
    if !ordered {
        v.sort_by_key(|r| r.to_string());
    }
    v
}

fn want_rows(expect: &Value) -> Vec<Value> {
    expect["rows"].as_array().cloned().unwrap_or_default()
}

/// Describe the gap between what came back and what was wanted, in terms of
/// rows — never in terms of the query that produced them.
fn describe_gap(got: &[Value], wanted: &[Value], columns: &[String]) -> String {
    let key = |v: &Value| v.to_string();
    let gk: Vec<String> = got.iter().map(key).collect();
    let wk: Vec<String> = wanted.iter().map(key).collect();

    let extra: Vec<&Value> = got.iter().filter(|r| !wk.contains(&key(r))).collect();
    let missing: Vec<&Value> = wanted.iter().filter(|r| !gk.contains(&key(r))).collect();

    let mut out = format!(
        "columns {:?}\n  returned {} row(s), wanted {}",
        columns,
        got.len(),
        wanted.len()
    );
    if !extra.is_empty() {
        out.push_str(&format!(
            "\n  rows returned that were not wanted: {extra:?}"
        ));
    }
    if !missing.is_empty() {
        out.push_str(&format!(
            "\n  rows wanted that were not returned: {missing:?}"
        ));
    }
    if extra.is_empty() && missing.is_empty() {
        out.push_str("\n  same rows, different order — the ordering is part of this want");
        out.push_str(&format!("\n  returned: {got:?}\n  wanted:   {wanted:?}"));
    }
    out
}

#[test]
fn every_query_satisfies_its_want() {
    let c = corpus();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for w in &c.wants {
        let id = w["id"].as_str().unwrap();
        let Some(expect) = w.get("expect") else {
            continue;
        };
        let kind = expect["kind"].as_str().unwrap();
        if kind == "relation" {
            continue; // handled below, from other wants' results
        }

        let got = match c.result(id) {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{id}: query did not run\n  {e}"));
                continue;
            }
        };
        checked += 1;

        match kind {
            "rows" => {
                let ordered = expect["ordered"].as_bool().unwrap();
                let columns: Vec<String> = expect["columns"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c.as_str().unwrap().to_string())
                    .collect();
                let g = normalise(&got, ordered);
                let mut wanted = want_rows(expect);
                if !ordered {
                    wanted.sort_by_key(|r| r.to_string());
                }
                if g != wanted {
                    failures.push(format!(
                        "{id}  ({})\n  {}",
                        w["intent"].as_str().unwrap_or(""),
                        describe_gap(&g, &wanted, &columns)
                    ));
                }
            }
            "count" => {
                // A count want is satisfied by a single scalar row.
                let n = got.as_array().and_then(|r| r.first()).and_then(|r| {
                    r.as_array()
                        .and_then(|c| c.first())
                        .and_then(|v| v.as_i64())
                });
                let wanted = expect["n"].as_i64();
                if n != wanted {
                    failures.push(format!("{id}: returned count {n:?}, wanted {wanted:?}"));
                }
            }
            other => failures.push(format!("{id}: unhandled expectation kind `{other}`")),
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {checked} queries did not satisfy their want.\n\
         The want is frozen: these queries are what is wrong.\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    println!("stage two: {checked} queries satisfy their frozen want");
}

#[test]
fn relations_between_wants_hold() {
    let c = corpus();
    let mut failures = Vec::new();

    for w in &c.wants {
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

        match op {
            // The parts must cover exactly `of` rows between them, with no
            // row counted twice — the check that a nullable column's `is
            // null` and `is not null` are exhaustive.
            "partition" | "sum_is" => {
                let of = expect
                    .get("of")
                    .or_else(|| expect.get("n"))
                    .and_then(|v| v.as_i64())
                    .unwrap();
                let mut total = 0i64;
                let mut seen: Vec<String> = Vec::new();
                for p in &parts {
                    let rows = c.result(p).unwrap_or_else(|e| panic!("{p}: {e}"));
                    let rows = rows.as_array().unwrap();
                    total += rows.len() as i64;
                    for r in rows {
                        seen.push(r.to_string());
                    }
                }
                let mut dedup = seen.clone();
                dedup.sort();
                dedup.dedup();
                if total != of {
                    failures.push(format!(
                        "{id}: {} together returned {total} rows, wanted {of}\n  {}",
                        parts.join(" + "),
                        w["intent"].as_str().unwrap_or("")
                    ));
                }
                if op == "partition" && dedup.len() != seen.len() {
                    failures.push(format!("{id}: the parts overlap — a row is in both"));
                }
            }
            // Two consecutive executions must be byte-identical, or the
            // ordering is not total and the corpus is testing luck.
            "stable_across_runs" => {
                for p in &parts {
                    let a = c.result(p).unwrap_or_else(|e| panic!("{p}: {e}"));
                    let b = c.result(p).unwrap_or_else(|e| panic!("{p}: {e}"));
                    if a != b {
                        failures.push(format!(
                            "{id}: {p} returned different rows on two consecutive runs"
                        ));
                    }
                }
            }
            other => failures.push(format!("{id}: unknown relation `{other}`")),
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn every_mechanical_want_has_a_query() {
    // A want with no query is not a pass, it is a gap. Naming them keeps the
    // corpus from looking complete because the missing ones are silent.
    let c = corpus();
    let missing: Vec<&str> = c
        .wants
        .iter()
        .filter(|w| {
            w.get("expect")
                .map(|e| e["kind"] != "relation")
                .unwrap_or(false)
        })
        .map(|w| w["id"].as_str().unwrap())
        .filter(|id| !c.sql.contains_key(*id))
        .collect();

    assert!(missing.is_empty(), "wants with no query yet: {missing:?}");
}
