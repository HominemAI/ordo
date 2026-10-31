//! Unit tests for the binder.
//!
//! These assert on the bound IR, not on SQL text. A test that greps the
//! planner's output for "LEFT JOIN" is testing two crates at once and will
//! point at the wrong one when it fails.

use ordo_schema::{build, Schema};
use ordo_sema::ir::{BCond, BExpr, From};
use ordo_sema::{bind, Query, Registry, SemaError};
use ordo_syntax::{parse_pipeline, parse_program};

const SRC: &str = r#"
entity vendors {
  name: text
}

entity products {
  vendor -> vendors
  alt    -> vendors?
  name:   text
  price:  money
  secret: text = "" sealed
  active: bool = true
}

op activeProducts =
  products(active == true)[name, price, vendor]

op cheap($cap: money) =
  activeProducts(price < $cap)[name, price]

op top2 =
  products[name, price]
  sort -price
  take 2

op byVendor =
  products[price, vendor]
  group vendor { total: sum price, n: count }

op selfCall =
  selfCall[name]
"#;

fn env() -> (Schema, Registry) {
    let p = parse_program(SRC).expect("fixture parses");
    (
        build(&p).expect("schema builds"),
        Registry::from_program(&p),
    )
}

fn q(src: &str) -> Query {
    let (s, r) = env();
    bind(
        &s,
        &r,
        &parse_pipeline(src).expect("pipeline parses"),
        ordo_sema::Clock::CORPUS,
    )
    .expect("binds")
}

fn err(src: &str) -> SemaError {
    let (s, r) = env();
    match parse_pipeline(src) {
        Ok(pl) => bind(&s, &r, &pl, ordo_sema::Clock::CORPUS).expect_err("expected a bind error"),
        Err(e) => panic!("expected a bind error, got a parse error: {}", e.message),
    }
}

fn out_names(q: &Query) -> Vec<&str> {
    q.outputs.iter().map(|o| o.name.as_str()).collect()
}

// ── sources and projection ───────────────────────────────────────────────

#[test]
fn an_omitted_projection_yields_the_open_declared_fields() {
    // Not the implicit ones — `id` and `rev` stay out because `[*]` means
    // what the author wrote. And not `secret` either: an entity with a
    // sealed field derives its read surface over the OPEN SKELETON, so the
    // field is absent rather than the whole entity being unreadable.
    let q = q("products");
    assert_eq!(out_names(&q), ["vendor", "alt", "name", "price", "active"]);
}

#[test]
fn star_and_omission_agree() {
    assert_eq!(out_names(&q("products[*]")), out_names(&q("products")));
}

#[test]
fn everything_minus_removes_exactly_one_field() {
    let q = q("products[*, -active]");
    assert_eq!(out_names(&q), ["vendor", "alt", "name", "price"]);
}

#[test]
fn star_offers_the_open_skeleton_and_naming_a_sealed_field_opts_in() {
    // The distinction that matters: "not offered" is not "unavailable". A
    // `*` that silently included the sealed column would be a disclosure
    // nobody asked for — the caller gets the open skeleton, and pays the
    // host-class cost only by naming the field.
    let star = q("products[*]");
    assert!(!out_names(&star).contains(&"secret"));
    assert!(star.host_open.is_empty(), "`*` costs nothing host-side");

    let named = q("products[secret]");
    assert_eq!(out_names(&named), ["secret"]);
    assert_eq!(named.host_open.len(), 1, "naming it opts into opening it");
}

/// Two sources may not project one output name (ruled 2026-10-05). Every
/// reader downstream resolves an output by name, and they disagreed about
/// which of two `name` columns that was. A rename keeps both.
#[test]
fn two_sources_projecting_one_name_are_refused() {
    for src in [
        "vendors[name] products[name]\nmatch products.vendor",
        "products[name] vendors[name]\nmatch products.vendor",
        "products[name, price] vendors[price: name]\nmatch products.alt",
    ] {
        let e = err(src);
        assert_eq!(e.code, "projection_unknown_field", "{src}: {}", e.message);
        assert!(e.message.contains("two sources"), "{src}: {}", e.message);
    }
    let q = q("vendors[name] products[product: name]\nmatch products.vendor");
    assert_eq!(out_names(&q), ["name", "product"]);
}

/// `*` on a filtered term offers what `*` on the plain term offers. The
/// filtered term is a sub-select carrying the entity's implicit `id` and
/// `rev` (fix 25s), and `*` through it answered them where the plain join
/// did not. A step still reaches them, and naming them still projects them.
#[test]
fn star_through_a_filtered_term_offers_the_plain_terms_shape() {
    // The many side.
    let plain = q("vendors[vname: name] products[*]\nmatch products.vendor");
    let filtered = q("vendors[vname: name] products(active == true)[*]\nmatch products.vendor");
    assert_eq!(
        out_names(&plain),
        ["vname", "vendor", "alt", "name", "price", "active"]
    );
    assert_eq!(out_names(&filtered), out_names(&plain));
    // The one side, whose join key is its `id`.
    let plain = q("products[pname: name] vendors[*]\nmatch products.vendor");
    let filtered = q("products[pname: name] vendors(name != \"x\")[*]\nmatch products.vendor");
    assert_eq!(out_names(&plain), ["pname", "name"]);
    assert_eq!(out_names(&filtered), out_names(&plain));
    // Named, the implicit fields project; in a step, they are reachable.
    let named = q("vendors[vname: name] products(active == true)[id, rev]\nmatch products.vendor");
    assert_eq!(out_names(&named), ["vname", "id", "rev"]);
    let stepped = q(
        "vendors[vname: name] products(active == true)[pname: name]\nmatch products.vendor\n| products.rev > 0",
    );
    assert_eq!(out_names(&stepped), ["vname", "pname"]);
}

#[test]
fn a_rename_changes_the_name_and_not_the_value() {
    let q = q("products[label: name]");
    assert_eq!(out_names(&q), ["label"]);
    assert!(matches!(&q.outputs[0].expr, BExpr::Col { column, .. } if column == "name"));
}

// ── composition ──────────────────────────────────────────────────────────

#[test]
fn a_plain_callee_merges_into_one_query() {
    // activeProducts has no take, group or distinct, so the caller's filter
    // can safely join the same WHERE without changing which rows exist.
    let q = q("activeProducts[name]");
    assert!(matches!(&q.from, From::Table { entity, .. } if entity == "products"));
    assert_eq!(q.filters.len(), 1, "the callee's own filter came along");
}

#[test]
fn a_settled_callee_becomes_a_subquery() {
    // top2 carries a take. A caller condition merged into it would filter
    // BEFORE the take and change which two rows were taken.
    let q = q("top2(price > 100)[name]");
    let From::Sub { query, .. } = &q.from else {
        panic!("expected a subquery, got a table: {:?}", q.from)
    };
    assert_eq!(query.limit, Some(ordo_sema::ir::Limit::Lit(2)));
    assert_eq!(
        q.filters.len(),
        1,
        "the caller's filter sits outside the take"
    );
    assert!(query.filters.is_empty(), "and did not leak inside it");
}

#[test]
fn a_grouped_callee_also_becomes_a_subquery() {
    let q = q("byVendor[total]");
    assert!(matches!(&q.from, From::Sub { .. }));
}

#[test]
fn a_settled_callee_carries_its_ordering_outward() {
    // With no sort of its own the caller keeps the callee's order, which is
    // only possible if the sort keys survive the wrap.
    let q = q("top2[name]");
    assert_eq!(q.sort.len(), 1, "callee sort key re-exposed");
    assert!(q.sort[0].desc);
    assert!(!q.tiebreak.is_empty(), "and so did the tiebreak");
}

#[test]
fn parameters_bind_at_the_call_site() {
    let q = q("cheap($cap: 5000)[name]");
    // The bound literal replaces the parameter outright, so no placeholder
    // survives into the plan.
    let has_param = q.filters.iter().any(|f| {
        matches!(
            f,
            BCond::Cmp {
                rhs: BExpr::Param { .. },
                ..
            }
        )
    });
    assert!(
        !has_param,
        "a bound parameter should not remain a placeholder"
    );
}

#[test]
fn composition_cycles_are_rejected() {
    assert_eq!(err("selfCall[name]").code, "cycle");
}

// ── joins ────────────────────────────────────────────────────────────────

#[test]
fn dotting_through_a_ref_adds_one_join() {
    let q = q("products[name, vendor.name]");
    assert_eq!(q.joins.len(), 1);
    assert_eq!(q.joins[0].table, "vendors");
    assert_eq!(q.joins[0].from_column, "vendor");
}

#[test]
fn two_paths_through_one_ref_share_a_join() {
    let q = q("products[a: vendor.name, b: vendor.name]");
    assert_eq!(q.joins.len(), 1, "the same hop must not be joined twice");
}

#[test]
fn parallel_refs_to_one_target_get_separate_joins() {
    // `vendor` and `alt` both point at vendors. Collapsing them would make
    // one row's two vendors the same vendor.
    let q = q("products[a: vendor.name, b: alt.name]");
    assert_eq!(q.joins.len(), 2);
    let mut cols: Vec<&str> = q.joins.iter().map(|j| j.from_column.as_str()).collect();
    cols.sort();
    assert_eq!(cols, ["alt", "vendor"]);
}

#[test]
fn dotting_a_non_ref_is_rejected() {
    assert_eq!(err("products[name.oops]").code, "not_a_ref");
}

// ── ordering ─────────────────────────────────────────────────────────────

#[test]
fn a_row_pipeline_tiebreaks_on_the_anchor_id() {
    let q = q("products[name]");
    assert_eq!(q.tiebreak.len(), 1);
    assert!(matches!(&q.tiebreak[0], BExpr::Col { column, .. } if column == "id"));
}

#[test]
fn a_grouped_pipeline_tiebreaks_on_its_group_keys() {
    // Grouping destroys row identity, so the anchor id is gone and the keys
    // take over — otherwise ties come back in server order.
    let (s, r) = env();
    let q = bind(
        &s,
        &r,
        &parse_pipeline("byVendor").expect("parses"),
        ordo_sema::Clock::CORPUS,
    )
    .expect("binds");
    let From::Sub { query, .. } = &q.from else {
        panic!("grouped callee wraps")
    };
    assert!(query.group.is_some());
    assert_eq!(query.tiebreak.len(), 1, "the group key is the tiebreak");
    assert!(!q.tiebreak.is_empty(), "and it survives the wrap");
}

// ── refusals ─────────────────────────────────────────────────────────────

#[test]
fn sealed_fields_cannot_be_read_in_any_position() {
    // Projection, filter, sort and star-expansion each reach a field by a
    // different path through the binder. A refusal that only covers the
    // obvious one is the failure mode worth testing for.
    // Every position from which a sealed field can actually be NAMED. `[*]`
    // is deliberately not in this list — see the open-skeleton test above.
    // PROJECTING a sealed field is legal now — it leaves as an envelope
    // and the host opens it. A rename is still a projection: `[copy:
    // secret]` discloses exactly what `[secret]` does.
    for src in [
        "products[secret]",
        "products[copy: secret]",
        "products[name, secret]",
    ] {
        let bound = q(src);
        assert_eq!(
            bound.host_open.len(),
            1,
            "{src} must carry its host-open obligation"
        );
    }
    // What refuses is any position where the DATABASE must reason about
    // the value — compare it, test it, or compute from it. It cannot read
    // what it cannot open, and doing the work host-side would mean
    // examining rows the caller never asked for.
    for src in [
        "products(secret == \"x\")[name]",
        "products(secret is null)[name]",
        "products[a: lower(secret)]",
        "products[name]\nsort secret",
    ] {
        assert_eq!(
            err(src).code,
            "sealed_unsupported",
            "sealed field reached through: {src}"
        );
    }
}

/// Audit 2026-10-09 finding 5. A `group` or `reduce` over rows that carry a
/// sealed field binds (ruled 2026-10-05: `count` reads no value). An
/// aggregate over the sealed value itself does not, because Postgres holds
/// only ciphertext and cannot sum, order or coalesce it. The refusal says
/// that and names what works; it used to blame `ordo-crypto` as unbuilt.
#[test]
fn an_aggregate_over_a_sealed_value_is_refused_and_says_what_works_instead() {
    let p = parse_program(
        "entity invoices {\n  issued: date\n  total: money(2) sealed\n}\n\
         \nop counted =\n  invoices[issued, total]\n  group issued { n: count }\n",
    )
    .expect("parses");
    let (s, r) = (
        build(&p).expect("schema builds"),
        Registry::from_program(&p),
    );
    let bind_src = |src: &str| {
        bind(
            &s,
            &r,
            &parse_pipeline(src).expect("pipeline parses"),
            ordo_sema::Clock::CORPUS,
        )
    };

    for src in [
        "invoices[issued, total]\ngroup issued { n: count }",
        "invoices[issued, total]\nreduce { n: count }",
    ] {
        let q = bind_src(src).unwrap_or_else(|e| panic!("{src}: {}", e.message));
        assert!(q.host_open.is_empty(), "{src}: a fold opens nothing");
    }

    for src in [
        "invoices[issued, total]\ngroup issued { s: sum total }",
        "invoices[issued, total]\ngroup issued { s: min total }",
        "invoices[issued, total]\ngroup issued { s: avg total }",
        "invoices[issued, total]\nreduce { s: sum total }",
        "invoices[issued, t: coalesce(total, 0)]",
    ] {
        let e = bind_src(src).expect_err(src);
        assert_eq!(e.code, "sealed_unsupported", "{src}: {}", e.message);
        assert!(
            e.message.contains("only its ciphertext")
                && e.message.contains("`count`")
                && e.message.contains("`sealed indexed`"),
            "{src}: {}",
            e.message
        );
        assert!(!e.message.contains("not built"), "{src}: {}", e.message);
    }
}

#[test]
fn a_bare_non_boolean_field_is_not_a_condition() {
    assert_eq!(err("products(name)").code, "non_boolean_condition");
    // A bool one is fine.
    assert!(q("products(active)[name]").filters.len() == 1);
}

#[test]
fn unknown_names_are_named() {
    assert_eq!(err("nope[a]").code, "unknown_source");
    // A miss is classed by the POSITION that named the field, and this one
    // is a projection: `projection_unknown_field` is the class the closed
    // inventory has for it. `unknown_field` is internal vocabulary and is
    // in no inventory, so it must not reach a conformance assertion (10f).
    assert_eq!(err("products[nope]").code, "projection_unknown_field");
    assert_eq!(err("cheap($nope: 1)[name]").code, "unknown_param");
    assert_eq!(err("cheap[name]").code, "unbound_required_param");
}

#[test]
fn unimplemented_constructs_are_refused_rather_than_mislowered() {
    // products carries TWO refs to vendors (vendor, alt) — parallel edges
    // are distinct paths, so this pair is ambiguous until `match <ref>`.
    assert_eq!(err("products[name]  vendors[name]").code, "ambiguous_match");
    // take[keys] is implemented; the ENTITY-QUALIFIED key spelling
    // (`take[products.id]`) is multi-source qualification and lands with
    // P5 — today the head resolves as a field and misses.
    assert_eq!(
        err("products[name]\ntake[products.id] 1").code,
        "unknown_field"
    );
    // The unqualified spelling works now: a keyed take wraps.
    let q1 = q("products[name]\ntake[vendor] 1");
    assert!(matches!(&q1.from, From::Sub { .. }));
}

#[test]
fn distinct_replaces_the_anchor_tiebreak_with_the_output_fields() {
    // `SELECT DISTINCT x ORDER BY id` is a Postgres error when id is not
    // projected — and semantically the anchor row is gone anyway. Found by
    // stage three of the corpus, not by review.
    let q = q("products[name]\ndistinct");
    assert!(q.distinct);
    assert_eq!(q.tiebreak.len(), 1);
    assert!(
        matches!(&q.tiebreak[0], BExpr::Col { column, .. } if column == "name"),
        "tiebreak must be the output field, not the vanished anchor id"
    );
}

// ── ruling W047: step keys need not be projected ─────────────────────────

#[test]
fn a_sort_key_is_inferred_from_the_source_row() {
    // `products[name] sort price` — price is not output, the source row
    // still exists, so the key resolves there [ruling W047].
    let q = q("products[name]\nsort price");
    assert_eq!(
        out_names(&q),
        ["name"],
        "the inferred key is not added to the output"
    );
    assert_eq!(q.sort.len(), 1);
    assert!(matches!(&q.sort[0].expr, BExpr::Col { column, .. } if column == "price"));
}

#[test]
fn a_filter_step_key_is_inferred_the_same_way() {
    let q = q("products[name]\n| price > 100");
    assert_eq!(out_names(&q), ["name"]);
    assert_eq!(q.filters.len(), 1);
}

#[test]
fn inference_reaches_through_refs_with_a_join() {
    let q = q("products[name]\nsort vendor.name");
    assert_eq!(q.joins.len(), 1, "the inferred dot-path still joins");
}

#[test]
fn a_group_key_and_aggregate_arg_are_inferred_too() {
    // No projection needed just to feed the group.
    let q = q("products\ngroup vendor { total: sum price }");
    let names = out_names(&q);
    assert_eq!(names, ["vendor", "total"]);
}

#[test]
fn the_fallback_dies_with_row_identity() {
    // After distinct the source row is gone; the key must be an output.
    let e = err("products[name]\ndistinct\nsort price");
    assert_eq!(e.code, "not_output_field");
    // Same after group.
    let e = err("products[vendor]\ngroup vendor { n: count }\nsort price");
    assert_eq!(e.code, "not_output_field");
}

#[test]
fn a_callee_caller_reaches_callee_outputs_but_never_deeper() {
    // top2 outputs [name, price]; the caller drops price but may still sort
    // by it — the callee's output row exists behind the caller's projection.
    let q = q("top2[name]\nsort price");
    assert_eq!(out_names(&q), ["name"]);
    // qty exists on products but the callee never output it: encapsulation
    // holds and inference must NOT tunnel into the base table.
    let e = err("activeProducts[name]\nsort qty");
    assert_eq!(e.code, "not_output_field");
}

#[test]
fn inference_never_reaches_a_sealed_field() {
    // The fallback widens what a step can see; it must not widen it to
    // secrets. Sorting by a sealed value is an ordering oracle for it.
    let e = err("products[name]\nsort secret");
    assert_eq!(e.code, "sealed_unsupported");
    let e = err("products[name]\n| secret == \"x\"");
    assert_eq!(e.code, "sealed_unsupported");
}

// ── antagonist findings, session review round 1 ──────────────────────────

// ── P5: two-source match inference ───────────────────────────────────────

#[test]
fn a_unique_direct_edge_joins_outer_preserving() {
    // top-level SRC: products.vendor -> vendors is one of TWO edges (alt
    // exists), so use a bespoke schema with a single edge.
    let p = parse_program(
        "entity teams { name: text }
entity people { team -> teams?
 name: text
 salary: money = 0 }",
    )
    .unwrap();
    let (s, r) = (build(&p).unwrap(), Registry::from_program(&p));
    let q = bind(
        &s,
        &r,
        &parse_pipeline(
            "teams[team: name]  people[]
group team { n: count }",
        )
        .unwrap(),
        ordo_sema::Clock::CORPUS,
    )
    .unwrap();
    assert_eq!(q.source_joins.len(), 1);
    assert!(
        q.source_joins[0].many_side,
        "people carries the ref: many side"
    );
    // Bare count was rebound to the many side's join key — COUNT(col)
    // self-masks, so the empty team counts 0, never 1.
    let n = q.outputs.iter().find(|o| o.name == "n").unwrap();
    assert!(
        matches!(&n.expr, BExpr::Agg { arg: Some(_), .. }),
        "bare count must count the deepest many-entity"
    );
}

#[test]
fn no_connecting_ref_is_no_match_path() {
    let p = parse_program(
        "entity a { x: text }
entity b { y: text }",
    )
    .unwrap();
    let (s, r) = (build(&p).unwrap(), Registry::from_program(&p));
    let e = bind(
        &s,
        &r,
        &parse_pipeline("a[x]  b[y]").unwrap(),
        ordo_sema::Clock::CORPUS,
    )
    .unwrap_err();
    assert_eq!(e.code, "no_match_path");
}

#[test]
fn the_same_entity_listed_twice_is_refused() {
    assert_eq!(err("products[name]  products[price]").code, "unsupported");
}

#[test]
fn ambiguous_unqualified_fields_demand_qualification() {
    let p = parse_program(
        "entity teams { name: text }
entity people { team -> teams?
 name: text }",
    )
    .unwrap();
    let (s, r) = (build(&p).unwrap(), Registry::from_program(&p));
    // `name` exists on both sources — unqualified is ambiguous_field,
    // qualified resolves.
    let e = bind(
        &s,
        &r,
        &parse_pipeline(
            "teams[]  people[]
sort name",
        )
        .unwrap(),
        ordo_sema::Clock::CORPUS,
    )
    .unwrap_err();
    assert_eq!(e.code, "ambiguous_field");
    let q = bind(
        &s,
        &r,
        &parse_pipeline(
            "teams[]  people[]
sort teams.name",
        )
        .unwrap(),
        ordo_sema::Clock::CORPUS,
    )
    .unwrap();
    assert_eq!(q.sort.len(), 1);
}

#[test]
fn a_merged_callees_sort_is_inherited_and_replaced_by_the_callers() {
    // Finding 1: activeProducts has a filter but cheap's shape here is a
    // merged callee; give it a sort via a wrapper-free path. `top2` is
    // settled; use a sorted-but-unsettled op instead.
    let p =
        parse_program("entity t { a: int\n b: int }\nop sorted =\n  t[a, b]\n  sort a").unwrap();
    let (s, r) = (build(&p).unwrap(), Registry::from_program(&p));
    let q = bind(
        &s,
        &r,
        &parse_pipeline("sorted[a, b]\nsort -b").unwrap(),
        ordo_sema::Clock::CORPUS,
    )
    .unwrap();
    assert_eq!(
        q.sort.len(),
        1,
        "caller sort replaced the callee's, not appended"
    );
    assert!(q.sort[0].desc);
}

#[test]
fn steps_after_take_operate_on_the_taken_rows() {
    // Finding 2: the take must settle before a later sort/filter/group.
    let q1 = q("products[name, price]\nsort -price\ntake 2\nsort price");
    let From::Sub { query, .. } = &q1.from else {
        panic!("take followed by a step must wrap")
    };
    assert_eq!(
        query.limit,
        Some(ordo_sema::ir::Limit::Lit(2)),
        "limit stays inside"
    );
    assert_eq!(q1.limit, None, "outer applies the later step");
    // Filter after take sits outside the limit too.
    let q2 = q("products[name, price]\ntake 2\n| price > 100");
    assert!(matches!(&q2.from, From::Sub { .. }));
    assert_eq!(q2.filters.len(), 1);
}

#[test]
fn a_prior_sort_key_must_be_an_output_when_rows_settle() {
    // Finding 3: this shape compiled into SELECT DISTINCT … ORDER BY an
    // unprojected column — a Postgres runtime error.
    let e = err("products[name]\nsort price\ndistinct");
    assert_eq!(e.code, "not_output_field");
    // A sort key that IS an output keeps its role through distinct.
    let q = q("products[name]\nsort name\ndistinct");
    assert_eq!(q.sort.len(), 1);
}

#[test]
fn no_new_join_after_group() {
    // Finding 4: the walk would land an ungrouped column in HAVING.
    let e = err("products[price, vendor]\ngroup vendor { t: sum price }\n| vendor.name == \"x\"");
    assert_eq!(e.code, "unsupported");
}

#[test]
fn null_and_fractional_money_cannot_bind() {
    // Finding 5: null typed as Text and planned `= NULL` — never true.
    assert_eq!(err("cheap($cap: null)[name]").code, "param_type_mismatch");
    // Finding 6, since 27ak: `money` here is `money(0)`, whole units, so
    // `29.99` would have to round to fit. A `money(2)` takes it as 2999
    // (`tests/money_places.rs`).
    let e = err("cheap($cap: 29.99)[name]");
    assert_eq!(e.code, "implicit_rounding");
    assert!(e.message.contains("money(0)"), "{}", e.message);
}

#[test]
fn match_ref_settles_parallel_edge_ambiguity() {
    // products carries TWO refs to vendors; bare listing is ambiguous
    // (tested above), and `match <ref>` picks the edge.
    let q1 = q("products[name]  vendors[vname: name]\nmatch products.vendor");
    assert_eq!(q1.source_joins.len(), 1);
    let q2 = q("products[name]  vendors[vname: name]\nmatch products.alt");
    assert_eq!(q2.source_joins.len(), 1);
    assert_ne!(
        q1.source_joins[0].on[0].1, q2.source_joins[0].on[0].1,
        "the two selections must join through different columns"
    );
}

#[test]
fn a_tree_entitys_own_subtree_is_id_under() {
    // `categories(id under $root)`-shaped: the tree's own id is a legal
    // under/above left-hand, not just refs pointing at the tree.
    let p = parse_program("entity cats { name: text\n parent -> cats? tree }").unwrap();
    let (s, r) = (build(&p).unwrap(), Registry::from_program(&p));
    let q = ordo_sema::bind_with_params(
        &s,
        &r,
        &parse_pipeline("cats(id under $root)[name]").unwrap(),
        &[("root".into(), ordo_syntax::parse_expr("\"c1\"").unwrap())],
        ordo_sema::Clock::CORPUS,
    )
    .unwrap();
    assert!(matches!(
        &q.filters[0],
        BCond::TreeWalk { above: false, .. }
    ));
}

// ── Stage 3b: guards inline as EXISTS semi-joins ─────────────────────────

const GSRC: &str = r#"
entity projects {
  name:  text
  owner -> members
  access read   owner == $caller or projectMembers(member == $caller)
  access write  owner == $caller
}

entity projectMembers {
  project -> projects  owned
  member  -> members   owned
  canWrite: bool = false
  access read   project.projectMembers(member == $caller)
  access write  project.owner == $caller
}

entity taskNotes2 {
  project -> projects  owned
  body: text
}

op openProjects unguarded =
  projects[name]
"#;

fn genv() -> (Schema, Registry) {
    let p = parse_program(GSRC).expect("parses");
    (build(&p).unwrap(), Registry::from_program(&p))
}

#[test]
fn read_guards_inline_on_entity_sources() {
    let (s, r) = genv();
    let q = ordo_sema::bind_as(
        &s,
        &r,
        &parse_pipeline("projects[name]").unwrap(),
        &[],
        Some("mem_1"),
        ordo_sema::Clock::CORPUS,
    )
    .unwrap();
    // owner == $caller OR EXISTS(projectMembers …): one Or filter.
    assert_eq!(q.filters.len(), 1);
    let BCond::Or(arms) = &q.filters[0] else {
        panic!("expected the guard's or: {:?}", q.filters[0])
    };
    assert!(matches!(arms[0], BCond::Cmp { .. }));
    assert!(matches!(arms[1], BCond::Exists { negated: false, .. }));
}

#[test]
fn inherited_guards_rebase_through_the_ownership_hop() {
    let (s, r) = genv();
    let q = ordo_sema::bind_as(
        &s,
        &r,
        &parse_pipeline("taskNotes2[body]").unwrap(),
        &[],
        Some("mem_1"),
        ordo_sema::Clock::CORPUS,
    )
    .unwrap();
    // The inherited walk wraps in EXISTS over the parent project.
    assert!(matches!(&q.filters[0], BCond::Exists { .. }));
}

#[test]
fn anonymous_makes_every_guard_false_by_rule() {
    let (s, r) = genv();
    let q = ordo_sema::bind_as(
        &s,
        &r,
        &parse_pipeline("projects[name]").unwrap(),
        &[],
        None,
        ordo_sema::Clock::CORPUS,
    )
    .unwrap();
    assert!(
        matches!(&q.filters[0], BCond::Bare(BExpr::Lit { .. })),
        "anonymous is FALSE, not null folklore: {:?}",
        q.filters[0]
    );
}

#[test]
fn unguarded_bypasses_body_sources_and_callee_guards_hold() {
    let (s, r) = genv();
    // The unguarded op's own body: no guard filter.
    let q = ordo_sema::bind_op(&s, &r, "openProjects", ordo_sema::Clock::CORPUS).unwrap();
    assert!(q.filters.is_empty(), "unguarded body reads unfiltered");
    // A guarded caller composing it does NOT re-guard the callee's body
    // (callee-body semantics, G13) — and its own sources still guard.
    let q2 = ordo_sema::bind_as(
        &s,
        &r,
        &parse_pipeline("openProjects[name]").unwrap(),
        &[],
        Some("mem_1"),
        ordo_sema::Clock::CORPUS,
    )
    .unwrap();
    assert!(
        q2.filters.is_empty(),
        "the callee's bypass is callee-scoped"
    );
}

// ── grouped lets joined back (reported fix 25at) ─────────────────────────

const LETS: &str = "entity vendors { name: text }
entity orders { vendor -> vendors
 status: text }
entity invoices { vendor -> vendors
 total: int }";

fn bind_lets(src: &str) -> Result<Query, SemaError> {
    let p = parse_program(LETS).unwrap();
    let (s, r) = (build(&p).unwrap(), Registry::from_program(&p));
    bind(
        &s,
        &r,
        &parse_pipeline(src).expect("pipeline parses"),
        ordo_sema::Clock::CORPUS,
    )
}

#[test]
fn two_lets_grouped_by_the_entity_id_join_back_one_to_one() {
    let q = bind_lets(
        "let o = vendors[id]  orders(status == \"done\")[]
        group vendors.id { done: count }
let i = vendors[id]  invoices[]
        group vendors.id { billed: count }
vendors[name]  o[done]  i[billed]",
    )
    .expect("a let keyed by vendors.id joins back to vendors");
    assert_eq!(q.source_joins.len(), 2);
    assert!(
        q.source_joins.iter().all(|j| !j.many_side),
        "each let holds one row per vendor, so neither join fans out"
    );
    assert_eq!(out_names(&q), ["name", "done", "billed"]);
}

#[test]
fn an_id_key_with_fellow_keys_from_the_same_row_stays_one_to_one() {
    bind_lets(
        "let o = vendors[id, name]  orders[]
        group vendors.id, vendors.name { done: count }
let i = vendors[id]  invoices[]
        group vendors.id { billed: count }
vendors[name]  o[done]  i[billed]",
    )
    .expect("vendors.name rides the id key, one row per vendor");
}

#[test]
fn two_lets_grouped_by_the_child_ref_join_back_one_to_one() {
    let q = bind_lets(
        "let o = orders[vendor] group vendor { done: count }
let i = invoices[vendor] group vendor { billed: count }
vendors[name]  o[done]  i[billed]",
    )
    .expect("a lone ref key holds one row per vendor");
    assert!(q.source_joins.iter().all(|j| !j.many_side));
}

#[test]
fn two_ungrouped_lets_on_one_entity_still_fan_out() {
    let e = bind_lets(
        "let o = orders[vendor, status]
let i = invoices[vendor, total]
vendors[name]  o[status]  i[total]",
    )
    .unwrap_err();
    assert_eq!(e.code, "fan_out", "{}", e.message);
}

#[test]
fn two_lets_keyed_by_more_than_the_ref_still_fan_out() {
    // `group vendor, status` is one row per (vendor, status): several per
    // vendor, so each let is a many-branch.
    let e = bind_lets(
        "let o = orders[vendor, status] group vendor, status { n: count }
let i = invoices[vendor, total] group vendor, total { m: count }
vendors[name]  o[n]  i[m]",
    )
    .unwrap_err();
    assert_eq!(e.code, "fan_out", "{}", e.message);
}

#[test]
fn two_many_entities_listed_directly_still_fan_out() {
    let e = bind_lets("vendors[name]  orders[status]  invoices[total]").unwrap_err();
    assert_eq!(e.code, "fan_out", "{}", e.message);
}

#[test]
fn a_let_with_no_key_back_names_the_source_it_cannot_reach() {
    let e = bind_lets(
        "let n = vendors[name] group name { k: count }
vendors[name]  n[k]",
    )
    .unwrap_err();
    assert_eq!(e.code, "no_match_path", "{}", e.message);
    assert!(
        e.message.contains("`vendors`") && e.message.contains("`n`"),
        "the refusal names both ends: {}",
        e.message
    );
}
