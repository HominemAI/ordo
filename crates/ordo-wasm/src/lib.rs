//! The Ordo compiler and the v0 decompiler, in a browser.
//!
//! This crate exists so the playground's converter can be a single static
//! HTML file with no server behind it: the page calls the **real** compiler,
//! compiled to WebAssembly, not a re-implementation of it in JavaScript. The
//! forward path is `ordo_syntax::parse_pipeline` → `ordo_sema::bind` →
//! `ordo_plan::plan`, reached through [`ordo_conformance::Fixture`], which is
//! the same constructor the conformance suite uses. The reverse path is
//! [`ordo_decompile::decompile`] plus its forward-verify.
//!
//! # What is NOT here, and why that is not a compromise
//!
//! Row execution. Every `run` in the served playground shells out to `psql`;
//! a browser has no Postgres and no subprocess, so the panes that execute
//! rows have no wasm counterpart. The functions below therefore answer the
//! compile-time half of `/api/forward` and `/api/reverse` and nothing else —
//! the SQL, the outputs, the cost class, the row bound, the decompilation,
//! its notes, and the round-trip SQL. That is a smaller claim than the
//! served playground makes, and the page must not make the larger one.
//!
//! Two consequences the page has to state rather than hide:
//!
//! - The forward pane shows the SQL a real Postgres *would* run. It does not
//!   show that Postgres accepted it. The conformance suite is what shows
//!   that, and it runs in CI, not here.
//! - The reverse pane verifies a decompilation by compiling it forward and
//!   showing both SQL texts side by side. The served playground goes one
//!   step further and runs BOTH queries, so a translation that returns
//!   different rows is caught. Without a database that check is impossible,
//!   so [`decompile`] returns the two SQL texts and says, in `notes_wasm`,
//!   that the row comparison did not happen. A reader comparing two SELECTs
//!   by eye is doing less than the differential test does, and should be
//!   told so rather than left to assume.
//!
//! # Calling convention
//!
//! Every export takes strings and returns one JSON string — never a JS
//! object graph. `JSON.parse` on the other side is one call and it keeps the
//! wasm/JS boundary to a single type, which is the boundary least likely to
//! rot. Errors are values: a refusal is `{"error": "..."}` with HTTP status
//! nowhere in the picture, exactly as the served handlers return it.
//!
//! ```js
//! import init, { compile, decompile, schema, default_fixture } from "./ordo_wasm.js";
//! await init(wasmBytes);
//! const fwd = JSON.parse(compile("", 'orders(status == "open")[total]'));
//! const rev = JSON.parse(decompile("", "SELECT name FROM vendors WHERE active = true"));
//! ```
//!
//! An empty `fixture_source` means "the bundled market fixture" — the same
//! `market.ordo` the conformance corpus is stated against. Passing your own
//! Ordo program compiles the query against that instead, which is what makes
//! the page a toy for your schema rather than only for ours.

use std::cell::RefCell;

use ordo_conformance::Fixture;
use serde_json::{json, Value};
use wasm_bindgen::prelude::wasm_bindgen;

/// The date every `since` window resolves against.
///
/// Identical to `ordo_playground::server::CLOCK`, and pinned for the same
/// reason: the fixture's rows sit in 2026-04/05, so a real clock makes every
/// window on them empty and teaches the reader that a working feature is
/// broken. The value is reported in the forward response so the page can
/// print it — a pinned clock nobody is told about is the bug the binder's
/// `Clock` argument exists to prevent, moved one layer out.
const CLOCK: ordo_sema::Clock = ordo_sema::Clock::CORPUS;

// ── the fixture cache ────────────────────────────────────────────────────

thread_local! {
    /// The last fixture compiled, keyed by its source text.
    ///
    /// Building one parses a 378-line program, derives the DDL, and runs the
    /// registration check. The page calls `compile` on every keystroke it
    /// feels like, almost always against the same fixture, so doing that work
    /// once per *distinct source* rather than once per call is the difference
    /// between a responsive pane and a stuttering one. A one-entry cache is
    /// enough: the page has one fixture box.
    static CACHE: RefCell<Option<(String, Fixture)>> = const { RefCell::new(None) };
}

/// Run `f` against the fixture for `src`, compiling it first if the cache
/// does not already hold it. A fixture that does not compile is an error
/// value, not a panic — the page's fixture box is editable.
fn with_fixture<T>(src: &str, f: impl FnOnce(&Fixture) -> T) -> Result<T, String> {
    let src = if src.trim().is_empty() {
        ordo_conformance::FIXTURE
    } else {
        src
    };
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.as_ref().map(|(k, _)| k.as_str()) != Some(src) {
            // `SEED` is the fixture's row data. Nothing here executes a row,
            // so it is never read — but `Fixture` stores a `&'static str` for
            // it and this is the only static there is.
            let fx = Fixture::from_source(src, ordo_conformance::SEED)?;
            *c = Some((src.to_string(), fx));
        }
        Ok(f(&c.as_ref().expect("just filled").1))
    })
}

// ── exports ──────────────────────────────────────────────────────────────

/// The bundled `market.ordo` — the fixture every conformance case is stated
/// against. Handed to the page so its fixture box can be pre-filled with
/// real source the reader can edit, rather than with a placeholder.
#[wasm_bindgen]
pub fn default_fixture() -> String {
    ordo_conformance::FIXTURE.to_string()
}

/// Ordo → SQL. Mirrors `/api/forward` minus everything that needs rows.
///
/// Returns `{"sql", "outputs", "cost", "rows_bound", "clock"}` on success and
/// `{"error"}` on any refusal, with the compiler's own message — parse, bind
/// and plan errors are all shaped the way the served playground shapes them,
/// because they come from the same `Fixture::compile`.
#[wasm_bindgen]
pub fn compile(fixture_source: &str, query: &str) -> String {
    let out = with_fixture(fixture_source, |f| forward(f, query))
        .unwrap_or_else(|e| json!({ "error": format!("fixture: {e}") }));
    out.to_string()
}

fn forward(f: &Fixture, source: &str) -> Value {
    // `whole entity("id")` is an execution-time read: it assembles a
    // document by running several queries against a database. There is no
    // compile-only half of it to show, so it is refused by name rather than
    // half-answered.
    if source.trim_start().starts_with("whole ") {
        return json!({
            "error": "`whole` assembles a document by executing queries, so it \
                      needs a database — this page compiles and plans, and has \
                      none. Run the served playground for `whole`."
        });
    }

    // The cost class and the row bound are read off the BOUND query, so they
    // are available even when planning later refuses. Same order as the
    // served handler, and for the same reason: a refusal that still tells
    // you the shape of what you asked for is worth more than a bare refusal.
    let bound_info = f.bind_case(source).ok().map(|q| {
        (
            ordo_plan::cost::classify_with(&q, Some(&f.schema)).as_str().to_string(),
            ordo_plan::cost::row_bound(&q, &f.schema).as_string(),
        )
    });

    match f.compile(source) {
        Err(e) => json!({ "error": e }),
        Ok(plan) => {
            let (y, m, d) = CLOCK.0;
            json!({
                "cost": bound_info.as_ref().map(|(c, _)| c.clone()),
                "rows_bound": bound_info.as_ref().map(|(_, b)| b.clone()),
                "clock": format!("{y:04}-{m:02}-{d:02}"),
                "sql": plan.sql,
                "outputs": plan.outputs.iter()
                    .map(|(n, t)| format!("{n}: {}", t.as_str()))
                    .collect::<Vec<_>>(),
            })
        }
    }
}

/// SQL → Ordo. Mirrors `/api/reverse` minus the row comparison.
///
/// Returns `{"ordo", "notes", "roundtrip_sql", "notes_wasm"}` on success,
/// `{"ordo", "notes", "verify_error"}` when the emitted Ordo does not compile
/// back (which is a bug in the decompiler and is shown as one), and
/// `{"error"}` when the SQL is outside the v0 subset.
#[wasm_bindgen]
pub fn decompile(fixture_source: &str, sql: &str) -> String {
    let out = with_fixture(fixture_source, |f| reverse(f, sql))
        .unwrap_or_else(|e| json!({ "error": format!("fixture: {e}") }));
    out.to_string()
}

fn reverse(f: &Fixture, sql: &str) -> Value {
    // The most common misfire on this pane is pasting Ordo into the SQL box;
    // the tokenizer then reports something true and useless. Recognise the
    // shape and say the helpful thing instead. Copied deliberately from
    // `ordo_playground::server::reverse` — the same conservative test: no
    // SELECT/WITH anywhere, and a construct only Ordo spells.
    {
        let t = sql.trim();
        let lower = t.to_lowercase();
        let has_sql_head =
            lower.contains("select") || lower.starts_with("with") || lower.starts_with("(");
        let ordo_marks = t.starts_with("op ")
            || t.starts_with("entity ")
            || t.starts_with("whole ")
            || t.contains("\nappend ")
            || t.starts_with("append ")
            || t.contains("\ntake ")
            || t.contains("\nsort ");
        if !has_sql_head && ordo_marks {
            return json!({
                "error": "this looks like Ordo, not SQL — this pane translates \
                          SQL into Ordo; paste Ordo source into the forward pane"
            });
        }
    }

    match ordo_decompile::decompile(&f.schema, sql) {
        Err(r) => json!({ "error": format!("outside the v0 subset: {}", r.what) }),
        Ok(d) => {
            let mut out = json!({ "ordo": d.ordo, "notes": d.notes });
            match f.compile(&d.ordo) {
                Err(e) => {
                    out["verify_error"] =
                        json!(format!("the decompilation does not compile back: {e}"));
                }
                Ok(plan) => {
                    out["roundtrip_sql"] = json!(plan.sql);
                    // Say what was NOT checked. The served playground runs
                    // both queries and reports a divergence; this page cannot,
                    // and a reader who has just been shown two SQL texts will
                    // otherwise assume the stronger check happened.
                    out["notes_wasm"] = json!([
                        "verified by compiling the emitted Ordo forward — the SQL shown \
                         is what it plans, to compare against the SQL you pasted. Their \
                         ROWS were not compared: that needs a database, and this page \
                         has none."
                    ]);
                }
            }
            out
        }
    }
}

/// The fixture's entities, their fields and its registered ops — what the
/// page needs to tell a reader which names the two boxes will accept.
/// Mirrors `/api/schema`.
#[wasm_bindgen]
pub fn schema(fixture_source: &str) -> String {
    let out = with_fixture(fixture_source, describe)
        .unwrap_or_else(|e| json!({ "error": format!("fixture: {e}") }));
    out.to_string()
}

fn describe(f: &Fixture) -> Value {
    let entities: Vec<Value> = f
        .schema
        .entities
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "platform": e.platform,
                "fields": e.declared().map(|fl| {
                    let ty = match &fl.ty {
                        ordo_schema::FieldTy::Scalar(s) => s.as_str().to_string(),
                        ordo_schema::FieldTy::Ref { target, .. } => format!("-> {target}"),
                    };
                    format!("{}: {ty}{}{}", fl.name,
                        if fl.optional { "?" } else { "" },
                        if fl.sealed { " (sealed)" } else { "" })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    let ops: Vec<String> = {
        let mut v: Vec<String> = f.registry.names().cloned().collect();
        v.sort();
        v
    };
    json!({ "entities": entities, "ops": ops })
}

// ── tests ────────────────────────────────────────────────────────────────
//
// These run on the HOST (`cargo test -p ordo-wasm`), not in a browser.
// `wasm_bindgen` exports are ordinary Rust functions on a native target, so
// the whole answer-shaping layer — which is the only code this crate owns —
// is testable without a headless browser in the loop. What they do NOT cover
// is that the wasm binary loads and links; `demo.html` and the Node harness
// beside it cover that.

#[cfg(test)]
mod tests {
    use super::*;

    fn j(s: String) -> Value {
        serde_json::from_str(&s).expect("valid json")
    }

    #[test]
    fn the_bundled_fixture_compiles_a_filtered_read() {
        let v = j(compile("", "orders(status == \"open\")[total]"));
        assert!(v["error"].is_null(), "{v}");
        let sql = v["sql"].as_str().expect("sql");
        assert!(sql.to_lowercase().starts_with("select"), "{sql}");
        assert_eq!(v["outputs"], json!(["total: money"]));
        assert!(v["cost"].is_string(), "{v}");
        assert_eq!(v["clock"], json!("2026-05-01"));
    }

    #[test]
    fn an_empty_fixture_argument_means_the_bundled_one() {
        assert_eq!(
            j(compile("", "activeProducts[name, price]")),
            j(compile(
                default_fixture().as_str(),
                "activeProducts[name, price]"
            ))
        );
    }

    #[test]
    fn a_refusal_comes_back_as_a_value_not_a_panic() {
        let v = j(compile("", "nosuchthing[x]"));
        assert!(v["error"].is_string(), "{v}");
        assert!(v["sql"].is_null(), "{v}");
    }

    #[test]
    fn a_fixture_that_does_not_compile_names_itself_as_the_cause() {
        let v = j(compile("entity oops {", "vendors[name]"));
        let e = v["error"].as_str().expect("error");
        assert!(e.starts_with("fixture:"), "{e}");
    }

    #[test]
    fn whole_is_refused_by_name_rather_than_half_answered() {
        let v = j(compile("", "whole orders(\"ord_1\")"));
        let e = v["error"].as_str().expect("error");
        assert!(e.contains("needs a database"), "{e}");
    }

    #[test]
    fn the_reverse_path_decompiles_and_verifies() {
        let v = j(decompile(
            "",
            "SELECT name FROM vendors WHERE active = true",
        ));
        assert!(v["error"].is_null(), "{v}");
        assert!(v["ordo"].as_str().expect("ordo").contains("vendors"), "{v}");
        assert!(v["roundtrip_sql"].is_string(), "{v}");
        assert!(v["verify_error"].is_null(), "{v}");
    }

    #[test]
    fn the_reverse_path_says_the_rows_were_not_compared() {
        let v = j(decompile(
            "",
            "SELECT name FROM vendors WHERE active = true",
        ));
        let n = v["notes_wasm"][0].as_str().expect("note");
        assert!(n.contains("ROWS were not compared"), "{n}");
    }

    #[test]
    fn ordo_pasted_into_the_sql_box_gets_the_helpful_answer() {
        let v = j(decompile("", "vendors[name]\nsort name"));
        let e = v["error"].as_str().expect("error");
        assert!(e.contains("looks like Ordo"), "{e}");
    }

    #[test]
    fn sql_outside_the_v0_subset_is_refused_by_name() {
        let v = j(decompile("", "SELECT name FROM vendors OFFSET 5"));
        let e = v["error"].as_str().expect("error");
        assert!(e.starts_with("outside the v0 subset:"), "{e}");
    }

    #[test]
    fn the_schema_export_lists_the_fixtures_entities_and_ops() {
        let v = j(schema(""));
        let names: Vec<&str> = v["entities"]
            .as_array()
            .expect("entities")
            .iter()
            .map(|e| e["name"].as_str().expect("name"))
            .collect();
        assert!(names.contains(&"orders"), "{names:?}");
        assert!(
            v["ops"]
                .as_array()
                .expect("ops")
                .contains(&json!("activeProducts")),
            "{v}"
        );
    }

    /// The cache is keyed by source text, so a second fixture must not be
    /// answered out of the first one's entry.
    #[test]
    fn switching_fixtures_switches_schemas() {
        let mine = "domain tiny @ 1.0.0\n\nentity widgets {\n  label: text\n}\n";
        assert!(j(compile(mine, "widgets[label]"))["sql"].is_string());
        assert!(j(compile("", "widgets[label]"))["error"].is_string());
        assert!(j(compile(mine, "widgets[label]"))["sql"].is_string());
    }
}
