//! Measure want quality by running independently-authored queries against the
//! hidden expectations.
//!
//! Usage:
//!   cargo run -p ordo-conformance --example blind_report -- <answers.json>...
//!
//! Each answers file is `{"queries": {"W001": "SELECT …"}}`, written by an
//! author who saw the intents and the schema but NOT the expected values and
//! NOT anybody else's queries.
//!
//! The number that matters is not how many queries were right. It is how many
//! wants an independent author could satisfy from the want's own words. A want
//! that only its own author can satisfy is a memo, not a specification, and
//! every one of those is a defect in the corpus rather than in the author.

use std::collections::BTreeMap;

use ordo_conformance::{Fixture, Pg};
use serde_json::Value;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: blind_report <answers.json>...");
        std::process::exit(2);
    }

    let wants: Value =
        serde_json::from_str(include_str!("../corpus/wants.json")).expect("wants.json");
    let fixture = Fixture::load_wide().expect("wide fixture");
    let pg = Pg::named("ordo_wide");
    fixture.reset(&pg).expect("seeded");

    // Author label -> want id -> query.
    let mut authors: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{f}: {e}"));
        let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{f}: {e}"));
        let label = std::path::Path::new(f)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let qs = v["queries"]
            .as_object()
            .expect("queries")
            .iter()
            .map(|(k, q)| (k.clone(), q.as_str().unwrap().to_string()))
            .collect();
        authors.push((label, qs));
    }

    let mut rows: Vec<(String, Vec<Option<bool>>)> = Vec::new();

    for w in wants["wants"].as_array().unwrap() {
        let Some(expect) = w.get("expect") else {
            continue;
        };
        if expect["kind"] == "relation" {
            continue;
        }
        let id = w["id"].as_str().unwrap().to_string();
        let mut results = Vec::new();

        for (_, qs) in &authors {
            // None = the author was not given this want; distinct from a
            // query that ran and missed.
            let ok = qs.get(&id).map(|sql| match fixture.run_sql(&pg, sql) {
                Err(_) => false,
                Ok(got) => satisfies(&got, expect),
            });
            results.push(ok);
        }
        rows.push((id, results));
    }

    let labels: Vec<&str> = authors.iter().map(|(l, _)| l.as_str()).collect();
    println!("want    {}", labels.join("  "));
    println!("{}", "-".repeat(8 + labels.len() * 12));

    let mut all_agree = 0usize;
    let mut none = Vec::new();
    let mut split = Vec::new();

    let mut skipped = 0usize;
    for (id, res) in &rows {
        let marks: Vec<String> = res
            .iter()
            .map(|ok| {
                format!(
                    "{:<10}",
                    match ok {
                        Some(true) => "satisfied",
                        Some(false) => "MISSED",
                        None => "—",
                    }
                )
            })
            .collect();
        println!("{id:<7} {}", marks.join("  "));
        // Tallies count only authors who attempted the want.
        let attempted: Vec<bool> = res.iter().filter_map(|x| *x).collect();
        if attempted.is_empty() {
            skipped += 1;
        } else if attempted.iter().all(|x| *x) {
            all_agree += 1;
        } else if attempted.iter().all(|x| !*x) {
            none.push(id.as_str());
        } else {
            split.push(id.as_str());
        }
    }

    println!();
    println!(
        "wants where every attempting author satisfied the hidden expectation: {all_agree}/{}",
        rows.len() - skipped
    );
    if skipped > 0 {
        println!("wants no listed author was given: {skipped}");
    }
    println!("wants no author satisfied: {} {none:?}", none.len());
    println!(
        "wants some satisfied and some did not: {} {split:?}",
        split.len()
    );
    println!();
    println!("A want that independent authors cannot satisfy from its own wording is");
    println!("under-specified. Those are corpus defects, and the rule is to retire and");
    println!("replace them — never to edit an expectation to match what came back.");
}

fn satisfies(got: &Value, expect: &Value) -> bool {
    match expect["kind"].as_str().unwrap() {
        "rows" => {
            let ordered = expect["ordered"].as_bool().unwrap();
            let mut g: Vec<Value> = got.as_array().cloned().unwrap_or_default();
            let mut w: Vec<Value> = expect["rows"].as_array().cloned().unwrap_or_default();
            if !ordered {
                g.sort_by_key(|r| r.to_string());
                w.sort_by_key(|r| r.to_string());
            }
            g == w
        }
        "count" => {
            let n = got.as_array().and_then(|r| r.first()).and_then(|r| {
                r.as_array()
                    .and_then(|c| c.first())
                    .and_then(|v| v.as_i64())
            });
            n == expect["n"].as_i64()
        }
        _ => false,
    }
}
