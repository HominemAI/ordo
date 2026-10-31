//! What the playground ANSWERS.
//!
//! These 300-odd lines of handler had no test at all, because they lived in
//! `main.rs` and a binary has no library surface. Each case here is one
//! thing a user of the page can see and be wrong about.
//!
//! One database for the whole binary, seeded once: the handlers read, they
//! do not mutate, so a per-test rebuild would be six `DROP SCHEMA`s racing
//! each other for nothing.

use ordo_playground::server::{self, App, ForwardReq, ManifestReq, ReverseReq};
use serde_json::Value;

/// This binary's own databases, rebuilt once.
///
/// Deliberately NOT the ones a running playground uses: two processes
/// sharing a database and both calling `reset` is the flake `Pg::named`
/// exists to stop, and a developer with the page open should not have it
/// truncated under them by `cargo test`.
fn app() -> &'static App {
    static APP: std::sync::OnceLock<App> = std::sync::OnceLock::new();
    APP.get_or_init(|| {
        let app = App::load()
            .expect("fixtures compile")
            .with_db_prefix("ordo_pg_srv");
        for (db, r) in app.seed() {
            r.unwrap_or_else(|e| panic!("could not seed {db}: {e}"));
        }
        app
    })
}

fn fwd(source: &str) -> Value {
    server::forward(
        app(),
        &ForwardReq {
            fixture: "market".into(),
            source: source.into(),
            ..Default::default()
        },
    )
}

fn run_as(fixture: &str, source: &str, caller: Option<&str>, page: Option<i64>) -> Value {
    server::forward(
        app(),
        &ForwardReq {
            fixture: fixture.into(),
            source: source.into(),
            run: true,
            caller: caller.map(str::to_string),
            page_size: page,
            ..Default::default()
        },
    )
}

fn names(rows: &Value, key: &str) -> Vec<String> {
    rows.as_array()
        .expect("rows")
        .iter()
        .map(|r| r[key].to_string())
        .collect()
}

// ── forward ─────────────────────────────────────────────────────────────

#[test]
fn a_compile_answers_with_sql_a_shape_and_a_cost() {
    let d = fwd("orders[placed, total]\nsort -total");
    assert!(d["error"].is_null(), "{d}");
    assert!(d["sql"].as_str().expect("sql").contains("FROM \"orders\""));
    assert_eq!(
        d["outputs"],
        serde_json::json!(["placed: date", "total: money"])
    );
    assert!(d["cost"].is_string(), "{d}");
    assert!(d["rows_bound"].is_string(), "{d}");
}

#[test]
fn a_refusal_comes_back_as_one_error_not_a_panic() {
    let d = fwd("orders[nosuchfield]");
    assert!(
        d["error"].as_str().expect("error").contains("nosuchfield"),
        "{d}"
    );
    assert!(d["sql"].is_null());
}

/// The clock `since` anchors to is pinned to the corpus date. That is a
/// defensible choice — a real clock makes every window on this seed empty —
/// but the answer has to SAY so, or the page silently shows a date-dependent
/// feature answering about a date the reader never chose.
#[test]
fn the_pinned_execution_clock_is_reported() {
    let d = fwd("orders[placed]");
    assert_eq!(d["clock"], "2026-05-01");
    // And it is the clock the plan actually used.
    let with_since = fwd("orders(placed since 90d)[placed]");
    let sql = with_since["sql"].as_str().expect("sql");
    assert!(
        sql.contains("2026-01-31"),
        "`since 90d` should anchor to the reported clock, got: {sql}"
    );
}

#[test]
fn a_caller_narrows_a_guarded_read_and_anonymous_sees_nothing() {
    // `tasks` is guarded by project membership. mem_1 is in proj_1 and
    // proj_3; anonymous is every guard false (G15).
    let mine = run_as("market", "allTasks[title]", Some("mem_1"), None);
    assert!(mine["run_error"].is_null(), "{mine}");
    let anon = run_as("market", "tasks[title]", None, None);
    assert_eq!(
        anon["rows"].as_array().expect("rows").len(),
        0,
        "an anonymous caller must see no guarded row: {anon}"
    );
    let seen = run_as("market", "tasks[title]", Some("mem_1"), None);
    assert!(
        !seen["rows"].as_array().expect("rows").is_empty(),
        "mem_1 is a project member and must see rows: {seen}"
    );
}

// ── paging ──────────────────────────────────────────────────────────────

#[test]
fn pages_partition_the_unpaged_answer_and_the_last_one_mints_no_cursor() {
    let src = "people[name, salary]\nsort -salary";
    let all = names(&run_as("wide", src, None, None)["rows"], "name");

    let mut seen = Vec::new();
    let mut after: Option<Vec<Option<String>>> = None;
    for _ in 0..10 {
        let d = server::forward(
            app(),
            &ForwardReq {
                fixture: "wide".into(),
                source: src.into(),
                run: true,
                page_size: Some(2),
                after: after.clone(),
                ..Default::default()
            },
        );
        assert!(d["run_error"].is_null(), "{d}");
        seen.extend(names(&d["rows"], "name"));
        match d["next"].as_array() {
            Some(_) => {
                after = serde_json::from_value(d["next"].clone()).expect("cursor");
            }
            None => break,
        }
    }
    assert_eq!(seen, all, "paging must reproduce the unpaged order exactly");
}

/// The paged path bound and planned but never resolved the blind-index
/// digest sites, so any query touching a `sealed indexed` field refused with
/// `unresolved_blind_index` the moment a page size was set — and ran fine
/// without one. Same query, two answers, decided by a dropdown.
#[test]
fn a_blind_indexed_lookup_pages_the_same_as_it_runs_unpaged() {
    let src = "customers(email == \"ada@example.com\")[name, tier]";
    let unpaged = run_as("market", src, None, None);
    assert!(unpaged["run_error"].is_null(), "{unpaged}");
    let paged = run_as("market", src, None, Some(10));
    assert!(
        paged["run_error"].is_null(),
        "the paged path must resolve blind sites too: {paged}"
    );
    assert_eq!(paged["rows"], unpaged["rows"]);
}

// ── money on the page ───────────────────────────────────────────────────

/// The playground showed money as stored minor units (`2999`) while the
/// door answers `"29.99"`. A reader copying a result from the page into a
/// client would have been off by the places. Ruled 2026-10-05, ask 1.
#[test]
fn a_money_result_reads_as_the_door_spells_it() {
    use ordo_syntax::ScalarTy;
    let outputs = vec![
        ("price".to_string(), ScalarTy::Money(2)),
        ("whole".to_string(), ScalarTy::Money(0)),
        ("rate".to_string(), ScalarTy::Decimal(4)),
        ("n".to_string(), ScalarTy::Int),
        ("name".to_string(), ScalarTy::Text),
    ];
    let rows = ordo_conformance::canonical(
        &[
            vec![
                Some("2999".into()),
                Some("2999".into()),
                Some("1.2500".into()),
                Some("7".into()),
                Some("2999".into()),
            ],
            vec![Some("-5".into()), None, None, None, None],
        ],
        &outputs,
    );
    let d = server::door_spelling(rows, &outputs);
    assert_eq!(
        d,
        serde_json::json!([
            {"price": "29.99", "whole": "2999", "rate": "1.2500", "n": 7, "name": "2999"},
            {"price": "-0.05", "whole": null, "rate": null, "n": null, "name": null},
        ])
    );
}

/// The same respelling reaches both run paths: unpaged and one page.
#[test]
fn a_money_column_is_a_string_on_the_page_paged_or_not() {
    let src = "orders[id, total]\nsort id";
    for page in [None, Some(2)] {
        let d = run_as("market", src, None, page);
        assert!(d["run_error"].is_null(), "{d}");
        let rows = d["rows"].as_array().expect("rows");
        assert!(!rows.is_empty(), "{d}");
        for r in rows {
            let t = r["total"].as_str().unwrap_or_else(|| panic!("{r}"));
            assert!(t.parse::<i64>().is_ok(), "money(0) spells whole units: {t}");
        }
    }
}

// ── whole ───────────────────────────────────────────────────────────────

#[test]
fn whole_runs_as_the_chosen_caller() {
    let mine = run_as("market", "whole projects(\"proj_1\")", Some("mem_1"), None);
    assert!(mine["error"].is_null(), "{mine}");
    assert!(mine["whole"].is_object(), "{mine}");
    // mem_4 is a member of no project: the anchor itself is not visible.
    let theirs = run_as("market", "whole projects(\"proj_1\")", Some("mem_4"), None);
    assert!(
        theirs["whole"].is_null() || theirs["error"].is_string(),
        "a non-member must not receive the document: {theirs}"
    );
}

// ── reverse ─────────────────────────────────────────────────────────────

fn rev(fixture: &str, sql: &str, run: bool) -> Value {
    server::reverse(
        app(),
        &ReverseReq {
            fixture: fixture.into(),
            sql: sql.into(),
            run,
        },
    )
}

/// The most common misfire on the reverse pane: pasting ORDO into the SQL
/// box. The tokenizer's answer was true and useless ("expected an
/// identifier, found Str(\"order\")"); the pane now recognises the shape
/// and says where the source belongs. Conservative on purpose — anything
/// with a SELECT head still goes to the decompiler.
#[test]
fn ordo_pasted_into_the_sql_pane_is_redirected_not_tokenized() {
    let d = rev(
        "market",
        "orders[kind: \"order\", label: status]\nappend coupons[kind: \"coupon\", label: code]\nsort kind\ntake 8",
        false,
    );
    let e = d["error"].as_str().expect("an error");
    assert!(e.contains("looks like Ordo"), "{e}");
    assert!(e.contains("forward pane"), "{e}");
    // A SELECT that merely *mentions* an Ordo-ish word is still SQL.
    let d = rev("wide", "SELECT name FROM people WHERE age > 40", false);
    assert!(d["error"].is_null(), "{d}");
}

#[test]
fn a_decompilation_is_verified_by_recompiling_it() {
    let d = rev("wide", "SELECT name FROM people WHERE age > 40", false);
    assert_eq!(d["ordo"], "people(age > 40)[name]");
    assert!(d["verify_error"].is_null(), "{d}");
    assert!(d["roundtrip_sql"].as_str().expect("sql").contains("people"));
}

/// The pane's whole claim. It printed the decompilation's rows and nothing
/// else, so a translation that returned different rows looked identical to
/// one that did not — on the one page built to expose the difference.
#[test]
fn running_a_decompilation_shows_both_sides() {
    let d = rev(
        "wide",
        "SELECT name FROM people WHERE nickname <> 'B'",
        true,
    );
    assert!(d["rows"].is_array(), "{d}");
    assert!(
        d["sql_rows"].is_array(),
        "your SQL's own rows must be on the page: {d}"
    );
    assert!(
        d["divergence"].is_null(),
        "this one agrees now; a divergence here means the null bridge broke: {d}"
    );
    assert_eq!(d["rows"].as_array().expect("rows").len(), 1);
}

#[test]
fn an_unprojected_group_key_no_longer_diverges_on_the_page() {
    // This case is here because it USED to be the crate's demonstration of a
    // divergence: Ordo's `group` always projects its keys, so the
    // decompilation came back with a column the SQL did not have, disclosed
    // in a note and printed under a `ROWS DIVERGE` banner. A `let` binding
    // takes the projection now, and the page shows two agreeing sides.
    let d = rev(
        "wide",
        "SELECT count(*) AS n FROM people p GROUP BY p.team",
        true,
    );
    assert!(
        d["ordo"]
            .as_str()
            .expect("ordo")
            .starts_with("let grouped ="),
        "{d}"
    );
    assert!(
        d["divergence"].is_null(),
        "the grouped translation is faithful now; a divergence means the `let` form broke: {d}"
    );
    assert_eq!(d["rows"].as_array().expect("rows").len(), 4);
}

#[test]
fn a_divergence_that_is_real_is_still_named_on_the_page() {
    // The banner has to still work, so something that genuinely differs has
    // to be run through it. `avg` is the honest candidate: Ordo pins scale
    // 12 so the answer cannot move between deployments, and Postgres derives
    // its scale from the operand — the same number, rendered with a
    // different number of decimal places. Disclosed in a note (no value
    // changes) and visible on the page (the text does).
    let d = rev(
        "wide",
        "SELECT p.org, avg(p.age) AS a FROM people p GROUP BY p.org",
        true,
    );
    assert!(
        d["notes"]
            .as_array()
            .expect("notes")
            .iter()
            .any(|n| n.as_str().unwrap_or("").contains("scale 12")),
        "{d}"
    );
    assert!(
        d["divergence"].is_string(),
        "differing rows must be named as such: {d}"
    );
}

/// The pane answer the owner's many-branch feed used to get, and gets now.
///
/// Before: `outside the v0 subset: expected an identifier, found
/// Str("order")` — a Rust `Debug` of a token, on the page whose job is to
/// make a translation visible. Now it is an `append`, verified by
/// recompiling, run on both sides, and agreeing.
#[test]
fn a_many_branch_feed_reaches_the_page_as_an_append() {
    let d = rev(
        "market",
        "SELECT 'order' AS kind, o.status AS title, o.total AS amount FROM orders o WHERE o.status = 'done' \
         UNION ALL \
         SELECT 'product', p.name, p.price FROM products p WHERE p.active = true \
         ORDER BY amount DESC LIMIT 100",
        true,
    );
    assert!(d["error"].is_null(), "{d}");
    assert_eq!(
        d["ordo"],
        "orders(status == \"done\")[kind: \"order\", title: status, amount: total]\n\
         append products(active == true)[kind: \"product\", title: name, amount: price]\n\
         sort -amount\n\
         take 100"
    );
    // Verified, not asserted: the emitted Ordo compiled back.
    assert!(d["verify_error"].is_null(), "{d}");
    assert!(
        d["roundtrip_sql"]
            .as_str()
            .expect("sql")
            .contains("UNION ALL"),
        "`append` is UNION ALL and the plan should say so: {d}"
    );
    // Both sides on the page, and agreeing.
    assert!(d["sql_rows"].is_array(), "{d}");
    assert!(
        d["divergence"].is_null(),
        "the feed translates faithfully; a divergence here means `append` broke: {d}"
    );
    assert_eq!(d["rows"].as_array().expect("rows").len(), 9);
    // The name-matching rule is DISCLOSED — a reader has to be able to see
    // why arm 2's `name` came back called `title`.
    assert!(
        d["notes"]
            .as_array()
            .expect("notes")
            .iter()
            .any(|n| n.as_str().unwrap_or("").contains("lines them up by NAME")),
        "{d}"
    );
}

/// The other half of the same pane: a construct with no Ordo spelling comes
/// back named, not tokenized.
#[test]
fn a_union_outside_the_subset_is_named_on_the_page() {
    let d = rev(
        "market",
        "SELECT o.status FROM orders o INTERSECT SELECT p.name FROM products p",
        false,
    );
    let e = d["error"].as_str().expect("an error");
    assert!(e.contains("INTERSECT"), "{e}");
    assert!(e.contains("no Ordo spelling"), "{e}");
    assert!(d["ordo"].is_null());
    // And a bind parameter — every pasted application query has one — says
    // what it is rather than dying in the tokenizer.
    let d = rev(
        "market",
        "SELECT o.status FROM orders o WHERE o.customer = $1",
        false,
    );
    let e = d["error"].as_str().expect("an error");
    assert!(e.contains("bind parameter"), "{e}");
}

#[test]
fn a_refusal_reaches_the_page_as_a_refusal() {
    let d = rev("wide", "SELECT name FROM people HAVING count(*) > 1", false);
    assert!(
        d["error"].as_str().expect("error").contains("HAVING"),
        "{d}"
    );
    assert!(d["ordo"].is_null());
}

#[test]
fn sql_that_does_not_run_is_reported_separately_from_ordo_that_does_not() {
    // Unquoted camelCase folds to lower case in Postgres; the decompilation
    // is fine and the submitted SQL is not, and the page must not blame the
    // wrong half.
    let d = rev(
        "wide",
        "SELECT name FROM people WHERE lastSeen IS NOT NULL",
        true,
    );
    assert!(d["sql_error"].is_string(), "{d}");
    assert!(d["rows"].is_array(), "{d}");
}

// ── manifest ────────────────────────────────────────────────────────────

fn man(op: &str) -> Value {
    server::manifest(
        app(),
        &ManifestReq {
            fixture: "market".into(),
            op: op.into(),
        },
    )
}

#[test]
fn an_unregistered_op_is_named_not_guessed() {
    assert!(man("nosuchop")["error"]
        .as_str()
        .expect("error")
        .contains("nosuchop"));
}

/// The acl derivation gained a second disjunct — the group walk — and the
/// manifest panel is what publishes a guard to whoever is being asked to
/// consent to it. A panel that renders only the first disjunct understates
/// who can see the rows.
#[test]
fn an_acl_derived_guard_publishes_its_group_disjunct() {
    let m = man("boardList");
    let guards = m["guards"].as_array().expect("guards");
    let boards = guards
        .iter()
        .find(|g| g["entity"] == "boards")
        .unwrap_or_else(|| panic!("no `boards` guard in {m}"));
    let read = boards["read"].as_str().expect("read");
    assert!(
        read.contains("member == $caller"),
        "the direct disjunct is missing: {read}"
    );
    assert!(
        read.contains("groupMembers"),
        "the group disjunct is missing from what the page publishes: {read}"
    );
    assert_eq!(boards["source"], "acl-derived");
}

#[test]
fn an_unguarded_op_discloses_what_it_bypasses() {
    let m = man("allTasks");
    assert_eq!(m["unguarded"], true, "{m}");
    assert!(
        !m["bypassed_guards"]
            .as_array()
            .expect("bypassed")
            .is_empty(),
        "the flag never travels alone: {m}"
    );
}

#[test]
fn the_manifest_carries_the_fields_a_caller_would_be_consenting_to() {
    let m = man("noteFeed");
    let scope = m["scope"].as_array().expect("scope");
    assert!(!scope.is_empty(), "{m}");
    assert!(scope.iter().all(|s| s["entity"].is_string()), "{m}");
}

// ── schema ──────────────────────────────────────────────────────────────

#[test]
fn the_schema_panel_marks_platform_entities_and_optional_fields() {
    let d = server::schema(app(), "market");
    let ents = d["entities"].as_array().expect("entities");
    let members = ents
        .iter()
        .find(|e| e["name"] == "members")
        .unwrap_or_else(|| panic!("`members` is a platform entity and should be listed"));
    assert_eq!(members["platform"], true);
    assert!(ents.iter().any(|e| e["name"] == "groupMembers"));
    // A domain entity is not mislabelled as one.
    assert_eq!(
        ents.iter().find(|e| e["name"] == "orders").expect("orders")["platform"],
        false
    );
    // Optionality is visible: `customers.email` is `text? sealed`.
    let customers = ents
        .iter()
        .find(|e| e["name"] == "customers")
        .expect("customers");
    let email = customers["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .find(|f| f.as_str().unwrap_or("").starts_with("email:"))
        .expect("email field");
    let email = email.as_str().unwrap();
    assert!(email.contains('?') && email.contains("sealed"), "{email}");
}

#[test]
fn the_schema_panel_lists_the_derived_std_ops_too() {
    let d = server::schema(app(), "market");
    let ops: Vec<&str> = d["ops"]
        .as_array()
        .expect("ops")
        .iter()
        .map(|o| o.as_str().unwrap())
        .collect();
    assert!(ops.contains(&"noteFeed"));
    assert!(ops.iter().any(|o| o.starts_with("std.")), "{ops:?}");
    assert!(ops.windows(2).all(|w| w[0] <= w[1]), "ops must be sorted");
}

// ── where the panes read from ───────────────────────────────────────────

/// Reads were served from `ordo_conformance` and `ordo_wide` — the
/// conformance suites' databases, which the playground never seeds and
/// which another `cargo test` may be dropping the schema of mid-read. Run
/// the playground on a tree where the suites had not run and every read
/// answered `relation "orders" does not exist`.
#[test]
fn every_default_database_belongs_to_the_playground() {
    let a = App::load().expect("fixtures compile");
    for db in [&a.market_db, &a.wide_db, &a.write_db] {
        assert!(
            db.starts_with("ordo_playground"),
            "`{db}` is not a database the playground owns and seeds"
        );
    }
    assert_eq!(a.pg("market").db, a.market_db);
    assert_eq!(a.pg("wide").db, a.wide_db);
    // Writes never land in a read database.
    assert_ne!(a.write_db, a.market_db);
}

// ── writes ──────────────────────────────────────────────────────────────

/// The write pane runs the real Stage-5 protocol and prints the command
/// group it appended. It had no test at all.
#[test]
fn a_write_appends_a_command_group_with_both_images() {
    let d = server::write(
        app(),
        &server::WriteReq {
            call: "renameTask($task: \"task_1\", $title: \"retitled\")".into(),
            caller: "mem_1".into(),
        },
    );
    assert!(d["error"].is_null(), "{d}");
    let cmds = d["group"]["commands"].as_array().expect("commands");
    assert_eq!(cmds.len(), 1, "{d}");
    assert_eq!(cmds[0]["entity"], "tasks");
    assert_eq!(cmds[0]["row"], "task_1");
    assert_eq!(cmds[0]["before"]["title"], "draft spec");
    assert_eq!(cmds[0]["after"]["title"], "retitled");
}

#[test]
fn a_write_the_guard_refuses_comes_back_as_one_error() {
    // mem_4 is in no project, so the write guard on `tasks` is false.
    let d = server::write(
        app(),
        &server::WriteReq {
            call: "renameTask($task: \"task_3\", $title: \"nope\")".into(),
            caller: "mem_4".into(),
        },
    );
    assert!(d["error"].is_string(), "a non-member must be refused: {d}");
    assert!(d["group"].is_null());
}
