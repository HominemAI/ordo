//! The reverse compiler's oracle: the stage-two SQL corpus.
//!
//! Those 37 queries carry blind-validated row sets. For every one the v0
//! subset accepts: decompile it, compile the emitted Ordo forward, execute
//! BOTH against the seeded database, and demand identical rows. A supported
//! query that stops round-tripping fails loudly; the unsupported remainder
//! is printed, never silently skipped.

use ordo_conformance::{Fixture, Pg};
use ordo_playground::decompile::decompile;
use serde_json::Value;

const SQL: &str = include_str!("../../ordo-conformance/corpus/sql.json");

/// Queries the v0 subset is expected to handle. Growing this list is the
/// reverse compiler's burn-down.
const SUPPORTED: &[&str] = &[
    "W001", "W002", "W003", "W005", "W006", "W008", "W009", "W010", "W011", "W013", "W014", "W015",
    "W018", "W019", "W020", "W023", "W027", "W028", "W030", "W031", "W033", "W034", "W038", "W048",
    "W053", "W054", "W056", "W057", "W058",
];

#[test]
fn stage_two_sql_round_trips() {
    let f = Fixture::load_wide().expect("wide fixture");
    // Its OWN database. Four suites reset `ordo_wide`, and this one calls
    // `reset` unconditionally rather than through a once-guard, so sharing
    // it meant one suite truncating another's tables mid-run — a flake
    // that looked like a round-trip divergence and was not. `Pg::named`
    // exists to say that in one line, and creates the database on the way.
    let pg = Pg::named("ordo_pg_roundtrip");
    f.reset(&pg).expect("seeded");

    let corpus: Value = serde_json::from_str(SQL).expect("sql.json");
    let queries = corpus["queries"].as_object().unwrap();

    let mut failures = Vec::new();
    let mut unsupported = Vec::new();
    let mut ok = 0;

    for (id, sql) in queries {
        let sql = sql.as_str().unwrap();
        let supported = SUPPORTED.contains(&id.as_str());
        match decompile(&f.schema, sql) {
            Err(r) => {
                if supported {
                    failures.push(format!("{id}: expected to decompile, refused: {}", r.what));
                } else {
                    unsupported.push(format!("{id} ({})", r.what));
                }
            }
            Ok(d) => {
                // Rows from the original SQL...
                let sql_rows = f.run_sql(&pg, sql).expect("original SQL runs");
                // ...must equal rows from the recompiled Ordo.
                match f.run_case(&pg, &d.ordo) {
                    Err(e) => failures.push(format!(
                        "{id}: decompiled but does not compile back\n  ordo: {}\n  {e}",
                        d.ordo.replace('\n', " ⏎ ")
                    )),
                    Ok(ordo_rows) => {
                        // run_case yields objects; run_sql yields arrays.
                        // Compare positionally via the object values in
                        // key order of the plan's outputs — cheapest fair
                        // comparison is multiset of value-lists.
                        let ordo_vals: Vec<String> = {
                            let mut v: Vec<String> = ordo_rows
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|o| {
                                    let m = o.as_object().unwrap();
                                    let mut vals: Vec<String> =
                                        m.values().map(|x| x.to_string()).collect();
                                    vals.sort();
                                    vals.join("|")
                                })
                                .collect();
                            v.sort();
                            v
                        };
                        let sql_vals: Vec<String> = {
                            let mut v: Vec<String> = sql_rows
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|r| {
                                    let mut vals: Vec<String> = r
                                        .as_array()
                                        .unwrap()
                                        .iter()
                                        .map(|x| x.to_string())
                                        .collect();
                                    vals.sort();
                                    vals.join("|")
                                })
                                .collect();
                            v.sort();
                            v
                        };
                        if ordo_vals == sql_vals {
                            ok += 1;
                        } else {
                            failures.push(format!(
                                "{id}: ROUND-TRIP DIVERGENCE\n  ordo: {}\n  sql rows:  {}\n  ordo rows: {}",
                                d.ordo.replace('\n', " ⏎ "),
                                sql_vals.join(" · "),
                                ordo_vals.join(" · ")
                            ));
                        }
                    }
                }
            }
        }
    }

    println!(
        "reverse compiler v0: {ok} of {} stage-two queries round-trip",
        queries.len()
    );
    println!("outside the subset: {} {unsupported:?}", unsupported.len());
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
    assert!(
        ok >= SUPPORTED.len(),
        "supported list promises {} but only {ok} round-tripped",
        SUPPORTED.len()
    );
}
