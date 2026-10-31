//! 9h — **the negative**: a statement that reaches a base table without
//! confining it to a space must not be constructible.
//!
//! The isolation suite (`ordo-conformance/tests/spaces.rs`) proves the
//! predicate is EMITTED and that it WORKS, by seeding two tenants and
//! showing each sees only its own. This file proves the other half: that
//! forgetting it is caught rather than shipped.
//!
//! Three layers, weakest last:
//!
//! 1. **The predicate is minted with the source.** `Planner::table` is the
//!    only function in the crate that renders a base table name, and it
//!    returns the source and its predicate as one value — there is nothing
//!    to take without the other.
//! 2. **Every emitted statement is verified.** `plan`, `plan_paged` and
//!    `plan_whole_guarded` run `verify_scoped` before returning, so a
//!    source that came from anywhere else, or a predicate that went
//!    missing, is a planner refusal rather than a cross-tenant read.
//! 3. **This file poses the failures directly**, because a checker nobody
//!    has seen fail is a checker nobody knows is wired up.

use ordo_plan::{plan, verify_whole, Space};
use ordo_schema::build;
use ordo_sema::{bind, Registry};
use ordo_syntax::{parse_pipeline, parse_program};

const SRC: &str = r#"
entity vendors { name: text }
entity categories {
  name: text
  parent -> categories? tree
}
entity products {
  vendor   -> vendors
  category -> categories
  name:  text
  price: money
}
entity notes {
  owner -> members
  body: text
  access read  owner == $caller
  access write owner == $caller
}
entity boards {
  name: text
  access read  shares(member == $caller)
  access write shares(member == $caller)
}
entity shares {
  board  -> boards
  member -> members
}
"#;

fn sql_of(pipeline: &str) -> String {
    let program = parse_program(SRC).expect("parses");
    let schema = build(&program).expect("schema");
    let registry = Registry::from_program(&program);
    let pl = parse_pipeline(pipeline).expect("parses");
    let q = bind(&schema, &registry, &pl, ordo_sema::Clock::CORPUS).expect("binds");
    plan(&q, &Space::bound("s1")).expect("plans").sql
}

/// Every `"name" AS "alias"` in a source position, paired with whether the
/// statement constrains that alias's space. Written here rather than
/// borrowed from the crate on purpose: a test that reuses the checker it is
/// testing proves only that the checker agrees with itself.
fn sources_and_scoping(sql: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for (i, _) in sql.match_indices(" AS \"") {
        // The quoted name immediately before ` AS `.
        let head = &sql[..i];
        if !head.ends_with('"') {
            continue;
        }
        let open = match head[..head.len() - 1].rfind('"') {
            Some(k) => k,
            None => continue,
        };
        let before = head[..open].chars().next_back().unwrap_or(' ');
        if !(before.is_whitespace() || before == '(' || before == ',') {
            continue;
        }
        if head[..open].trim_end().ends_with("COLLATE") {
            continue;
        }
        let tail = &sql[i + " AS \"".len()..];
        let alias = match tail.find('"') {
            Some(k) => &tail[..k],
            None => continue,
        };
        out.push((
            alias.to_string(),
            sql.contains(&format!("\"{alias}\".\"$space\"")),
        ));
    }
    out
}

/// Every source in an ordinary read is scoped — the FROM, the dot-hop
/// join, the guard's sub-EXISTS, the subquery a settled callee becomes,
/// and both ends of a tree walk's self-join.
#[test]
fn every_source_a_plan_names_is_confined_to_a_space() {
    for pipeline in [
        "products[name, price]",
        "products[name, vendor.name]",
        "notes[body]",
        "products[name, price]\nsort price\ntake 2",
        "products(category under \"c1\")[name]",
        "products[name, price]\ngroup vendor { n: count }",
    ] {
        let sql = sql_of(pipeline);
        let sources = sources_and_scoping(&sql);
        assert!(
            !sources.is_empty(),
            "`{pipeline}` named no source at all, so this case tests nothing:\n{sql}"
        );
        for (alias, scoped) in &sources {
            // Derived aliases (subqueries, union wrappers) are scoped
            // inside; only base tables are named `"entity" AS "alias"`,
            // and the scan above finds exactly those.
            if !scoped {
                assert!(
                    sql.contains(&format!("\"{alias}\".\"$space\"")),
                    "`{pipeline}`: the source read as `{alias}` carries no space predicate:\n{sql}"
                );
            }
        }
    }
}

/// A tree walk's self-join is TWO sources, and scoping one is not scoping
/// the walk: a prefix test that reads another space's `$path` places rows
/// under an ancestor they do not have.
#[test]
fn both_ends_of_a_tree_walk_are_confined() {
    let sql = sql_of("products(category under \"c1\")[name]");
    assert!(sql.contains("\"n\".\"$space\" = 's1'"), "{sql}");
    assert!(sql.contains("\"anc\".\"$space\" = 's1'"), "{sql}");
}

/// A guard is compiled to a sub-EXISTS over a base table, and it is the
/// source most easily forgotten because nobody wrote it: the author sees
/// `access read shares(member == $caller)` and never sees the join.
#[test]
fn a_guards_sub_exists_is_confined() {
    let sql = sql_of("boards[name]");
    assert!(
        sql.contains("EXISTS"),
        "the guard must compile to a semi-join:\n{sql}"
    );
    for (alias, scoped) in sources_and_scoping(&sql) {
        assert!(
            scoped,
            "the guard's source `{alias}` is unconfined — a caller's grant in one space \
             would authorise another space's rows:\n{sql}"
        );
    }
    assert_eq!(
        sql.matches("\"$space\" = 's1'").count(),
        2,
        "one for the anchor, one for the guard body:\n{sql}"
    );
}

/// **The refusal.** A statement that names a base table without confining
/// it does not plan.
#[test]
fn an_unscoped_statement_is_refused() {
    let leaky = "SELECT \"t0\".\"name\" AS \"name\" FROM \"products\" AS \"t0\";";
    let e = verify_whole(leaky, &["t0".into()]).expect_err("must not verify");
    assert_eq!(e.code, "unscoped_source");
    assert!(e.message.contains("t0"), "{e}");

    // The same statement WITH the predicate verifies, so the refusal is
    // about the predicate and not about the shape.
    let ok = "SELECT \"t0\".\"name\" AS \"name\" FROM \"products\" AS \"t0\" \
              WHERE \"t0\".\"$space\" = 's1';";
    verify_whole(ok, &["t0".into()]).expect("verifies");
}

/// **The other refusal.** A source the planner never registered — a table
/// name rendered by hand, bypassing `Planner::table` — is caught even
/// though the statement happens to carry a predicate for some other alias.
#[test]
fn a_source_the_planner_never_registered_is_refused() {
    let smuggled = "SELECT 1 AS \"$e\" FROM \"products\" AS \"t0\", \"vendors\" AS \"t9\" \
                    WHERE \"t0\".\"$space\" = 's1';";
    let e = verify_whole(smuggled, &["t0".into()]).expect_err("must not verify");
    assert_eq!(e.code, "unscoped_source");
    assert!(e.message.contains("t9"), "{e}");
}

/// The artifact form: `$1` is the space in every prepared statement, and
/// it leads the bind order.
#[test]
fn the_reserved_slot_is_always_the_first_bind() {
    let program = parse_program(SRC).expect("parses");
    let schema = build(&program).expect("schema");
    let registry = Registry::from_program(&program);
    let pl = parse_pipeline("products[name, price]").expect("parses");
    let q = bind(&schema, &registry, &pl, ordo_sema::Clock::CORPUS).expect("binds");
    let p = plan(&q, &Space::Slot).expect("plans");
    assert_eq!(p.params.first().map(String::as_str), Some("$space"));
    assert!(p.sql.contains("\"t0\".\"$space\" = $1"), "{}", p.sql);
}

/// An op may not declare `$space`: the door binds it, and a parameter by
/// that name would be a second one the caller controls (SH2-1).
#[test]
fn an_op_may_not_declare_the_space_as_a_parameter() {
    let program = parse_program(
        "entity vendors { name: text }\nop bySpace($space: text) = vendors(name == $space)[name]",
    )
    .expect("parses");
    let schema = build(&program).expect("schema");
    let registry = Registry::from_program(&program);
    let e = ordo_plan::cost::check_registration(&schema, &registry)
        .expect_err("registration must refuse it");
    assert_eq!(e.code, "reserved_param");
}
