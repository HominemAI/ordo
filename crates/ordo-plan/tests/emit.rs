//! Unit tests for SQL emission.
//!
//! The planner's contract is narrow: everything it prints was already
//! resolved, so its only jobs are to be deterministic and to be honest about
//! ordering. These test those two things, plus the operator and literal
//! mappings where a wrong character is a wrong answer rather than a syntax
//! error — `+` on text, integer division, `<>` for inequality.

use ordo_plan::{plan, Space};
use ordo_schema::build;
use ordo_sema::{bind, Registry};
use ordo_syntax::{parse_pipeline, parse_program};

/// The space these unit tests plan in. There is no default: `plan` takes
/// one, so a test that did not care still has to say which.
fn test_space() -> Space {
    Space::bound("s1")
}

const SRC: &str = r#"
entity vendors {
  name: text
}

entity products {
  vendor -> vendors
  name:   text
  price:  money
  qty:    int
  active: bool = true
  due:    date?
}

op top2 =
  products[name, price]
  sort -price
  take 2

op byVendor =
  products[price, vendor]
  group vendor { total: sum price, n: count }
"#;

fn sql(src: &str) -> String {
    let p = parse_program(SRC).expect("fixture parses");
    let schema = build(&p).expect("schema builds");
    let registry = Registry::from_program(&p);
    let q = bind(
        &schema,
        &registry,
        &parse_pipeline(src).expect("parses"),
        ordo_sema::Clock::CORPUS,
    )
    .expect("binds");
    plan(&q, &test_space()).expect("plans").sql
}

// Sort keys need not be projected [ruling W047]: steps resolve against
// outputs first, then the pre-projection source scope. The pipelines below
// mostly project their keys for readability, not necessity — see
// `an_unprojected_sort_key_reaches_the_source_column`.

/// The ORDER BY of the outermost SELECT.
fn outer_order(sql: &str) -> String {
    sql.rsplit_once("ORDER BY")
        .map(|(_, o)| o.trim().trim_end_matches(';').to_string())
        .unwrap_or_default()
}

// ── ordering: the property the corpus depends on ─────────────────────────

#[test]
fn every_plan_ends_in_a_total_order() {
    for src in [
        "products[name]",
        "products[name, price]\nsort price",
        "products[name, price]\nsort -price",
        "top2[name]",
        "byVendor",
        "products[name, vendor.name]\nsort name",
    ] {
        let s = sql(src);
        assert!(s.contains("ORDER BY"), "no ordering at all in:\n{s}");
        // Two keys minimum whenever a sort was declared: the declared key
        // cannot be assumed unique.
        let order = outer_order(&s);
        assert!(
            !order.is_empty(),
            "the outermost select has no ordering in:\n{s}"
        );
    }
}

#[test]
fn a_declared_sort_is_followed_by_the_tiebreak() {
    let order = outer_order(&sql("products[name, price]\nsort price"));
    let price = order.find("price").expect("declared key present");
    let id = order.find("\"id\"").expect("tiebreak present");
    assert!(price < id, "the declared key must come first: {order}");
}

#[test]
fn a_key_is_never_repeated_in_the_ordering() {
    // Sorting by the anchor explicitly must not emit it twice.
    let order = outer_order(&sql("products[name, id]\nsort id"));
    assert_eq!(order.matches("\"id\"").count(), 1, "{order}");
}

#[test]
fn null_placement_is_explicit_in_both_directions() {
    // Postgres defaults NULLS LAST for ASC and NULLS FIRST for DESC. Relying
    // on that would silently move nulls when a sort is reversed.
    for src in [
        "products[name, due]\nsort due",
        "products[name, due]\nsort -due",
    ] {
        let order = outer_order(&sql(src));
        assert!(order.contains("NULLS LAST"), "{order}");
        assert!(!order.contains("NULLS FIRST"), "{order}");
    }
}

// ── determinism ──────────────────────────────────────────────────────────

#[test]
fn planning_is_deterministic() {
    // Aliases come from a counter and joins from a list; a set or a map
    // anywhere in that path would make golden files churn.
    let a = sql("products[name, price, vendor.name]\nsort price");
    for _ in 0..5 {
        assert_eq!(a, sql("products[name, price, vendor.name]\nsort price"));
    }
}

// ── structure ────────────────────────────────────────────────────────────

#[test]
fn identifiers_are_quoted_everywhere() {
    let s = sql("products[name, vendor.name]");
    assert!(s.contains(r#""products" AS "t0""#), "{s}");
    assert!(s.contains(r#"LEFT JOIN "vendors" AS "t1""#), "{s}");
}

#[test]
fn a_ref_hop_is_a_left_join_not_an_inner_one() {
    // Dotting through a ref must not drop the row whose ref is null.
    let s = sql("products[name, vendor.name]");
    assert!(s.contains("LEFT JOIN"), "{s}");
    assert_eq!(s.matches("JOIN").count(), 1, "exactly one join:\n{s}");
}

#[test]
fn a_settled_callee_becomes_a_nested_select() {
    let s = sql("top2(price > 100)[name]");
    assert_eq!(s.matches("SELECT").count(), 2, "{s}");
    assert!(s.contains("LIMIT 2"), "{s}");
    // The caller's filter belongs outside the LIMIT, or it changes which
    // rows the take took.
    let limit = s.find("LIMIT 2").unwrap();
    let last_where = s.rfind("WHERE").unwrap();
    assert!(
        last_where > limit,
        "caller filter leaked inside the take:\n{s}"
    );
}

#[test]
fn grouping_emits_its_keys() {
    let s = sql("byVendor");
    assert!(s.contains("GROUP BY"), "{s}");
    assert!(s.contains("sum("), "{s}");
    assert!(s.contains("count(*)"), "{s}");
}

// ── operators and literals: a wrong character is a wrong answer ──────────

#[test]
fn plus_is_concatenation_on_text_and_addition_on_numbers() {
    // `+` doubles as concatenation in the source grammar; the inferred type
    // is what decides which SQL operator it becomes.
    assert!(sql("products[a: name + \"x\"]").contains("||"));
    assert!(sql("products[a: qty + 1]").contains(" + "));
}

#[test]
fn inequality_keeps_two_valued_logic() {
    // DL §2's "two-valued logic is a compilation obligation [settled]":
    // `NOT (a = b)` under SQL's 3VL is UNKNOWN on a null and the row is
    // silently dropped — the opposite of `null != 5` = true. A plain `<>` is
    // that bug wherever a side can be null, and only there.
    //
    // `qty` is `NOT NULL` and `1` is a literal: neither can be null, so
    // `<>` is already two-valued.
    let s = sql("products(qty != 1)[name]");
    assert!(s.contains(r#"("t0"."qty" <> 1)"#), "{s}");
    // `due` is optional: the null case is spelled out.
    let s = sql("products(due != 2026-01-01)[name]");
    assert!(
        s.contains(r#"(NOT ("t0"."due" = DATE '2026-01-01') OR "t0"."due" IS NULL)"#),
        "{s}"
    );
    // Both sides optional: the distinctness form, unchanged.
    let s = sql("products(due != due)[name]");
    assert!(
        s.contains(r#"("t0"."due" IS DISTINCT FROM "t0"."due")"#),
        "{s}"
    );
    assert!(!s.contains("!="), "{s}");
}

/// The guard on the never-null rule (owner ruling 2026-10-26,
/// `guard-equality-uses-no-index`): a bare `a = b` is emitted only with a
/// side proven never null. Each case below has no proven side.
#[test]
fn plain_equality_needs_a_side_proven_never_null() {
    for (src, want) in [
        // Two optional columns.
        (
            "products(due == due)[name]",
            r#"("t0"."due" = "t0"."due" OR ("t0"."due" IS NULL AND "t0"."due" IS NULL))"#,
        ),
        // Under `not`, only two `NOT NULL` columns qualify: an optional
        // column, or a literal, keeps the distinctness form.
        (
            "products(not (qty == due))[name]",
            r#"(NOT ("t0"."qty" IS NOT DISTINCT FROM "t0"."due"))"#,
        ),
        (
            "products(not (name == \"x\"))[name]",
            r#"(NOT ("t0"."name" IS NOT DISTINCT FROM 'x'))"#,
        ),
    ] {
        let s = sql(src);
        assert!(s.contains(want), "{src}:\n{s}");
    }

    // A query that carries no `NOT NULL` evidence proves no column: a
    // literal side still counts, two columns do not.
    let p = parse_program(SRC).expect("fixture parses");
    let schema = build(&p).expect("schema builds");
    let registry = Registry::from_program(&p);
    let mut q = bind(
        &schema,
        &registry,
        &parse_pipeline("products(qty == 1 and vendor.name == name)[name]").expect("parses"),
        ordo_sema::Clock::CORPUS,
    )
    .expect("binds");
    let with = plan(&q, &test_space()).expect("plans").sql;
    assert!(with.contains(r#"("t0"."qty" = 1)"#), "{with}");
    assert!(with.contains(r#"("t1"."name" = "t0"."name")"#), "{with}");
    q.not_null = None;
    let without = plan(&q, &test_space()).expect("plans").sql;
    assert!(without.contains(r#"("t0"."qty" = 1)"#), "{without}");
    assert!(
        without.contains(
            r#"("t1"."name" = "t0"."name" OR ("t1"."name" IS NULL AND "t0"."name" IS NULL))"#
        ),
        "{without}"
    );
}

/// [`ordo_plan::never_null`] on its own: what proves an expression never
/// null, and what does not.
#[test]
fn never_null_needs_evidence_for_every_part() {
    use ordo_plan::{never_null, NeverNull, NullScope};
    use ordo_sema::BExpr;
    use ordo_syntax::decl::ScalarTy;
    let col = |alias: &str, column: &str| BExpr::Col {
        alias: alias.into(),
        column: column.into(),
        ty: ScalarTy::Text,
    };
    let not_null: std::collections::BTreeMap<_, _> = [(
        "products".to_string(),
        ["id", "name"].map(String::from).into_iter().collect(),
    )]
    .into_iter()
    .collect();
    let present = [("t0".to_string(), "products".to_string())];
    let scope = NullScope {
        not_null: Some(&not_null),
        present: &present,
    };
    assert_eq!(
        never_null(&col("t0", "name"), &scope),
        Some(NeverNull::Column)
    );
    // An optional column, a column of a row that may be absent (an outer
    // join's padding), and any column with no evidence at all.
    assert_eq!(never_null(&col("t0", "due"), &scope), None);
    assert_eq!(never_null(&col("t1", "name"), &scope), None);
    let blind = NullScope {
        not_null: None,
        present: &present,
    };
    assert_eq!(never_null(&col("t0", "name"), &blind), None);
    // `null`, an ordinary parameter, `$caller`.
    let null = BExpr::Lit {
        value: ordo_syntax::Expr::NullLit {
            span: ordo_syntax::span::Span::new(0, 0, 1, 1),
        },
        ty: ScalarTy::Text,
    };
    assert_eq!(never_null(&null, &scope), None);
    let param = |name: &str| BExpr::Param {
        name: name.into(),
        ty: ScalarTy::Id,
    };
    assert_eq!(never_null(&param("d"), &scope), None);
    assert_eq!(never_null(&param("caller"), &scope), Some(NeverNull::Value));
    // An operation is never null only when every input is.
    let lower = |arg: BExpr| BExpr::Fn {
        name: "lower".into(),
        args: vec![arg],
        ty: ScalarTy::Text,
    };
    assert_eq!(
        never_null(&lower(col("t0", "name")), &scope),
        Some(NeverNull::Value)
    );
    assert_eq!(never_null(&lower(col("t0", "due")), &scope), None);
    let coalesce = BExpr::Fn {
        name: "coalesce".into(),
        args: vec![col("t0", "due"), col("t0", "name")],
        ty: ScalarTy::Text,
    };
    assert_eq!(never_null(&coalesce, &scope), Some(NeverNull::Value));
}

#[test]
fn text_operators_do_not_become_patterns() {
    // `contains` takes a value, not a pattern. Lowering it to LIKE would give
    // `%` and `_` a meaning the language does not have.
    let s = sql("products(name contains \"a%b\")[name]");
    assert!(s.contains("strpos("), "{s}");
    assert!(!s.to_uppercase().contains("LIKE"), "{s}");
}

#[test]
fn string_literals_are_escaped() {
    let s = sql("products(name == \"it's\")[name]");
    assert!(s.contains("'it''s'"), "{s}");
}

#[test]
fn date_literals_are_typed_not_stringly() {
    let s = sql("products(due >= 2026-01-01)[name]");
    assert!(s.contains("DATE '2026-01-01'"), "{s}");
}

#[test]
fn a_bare_boolean_is_compared_explicitly() {
    // `IS TRUE` rather than a bare column: a null bool must not pass a filter.
    let s = sql("products(active)[name]");
    assert!(s.contains("IS TRUE"), "{s}");
}

#[test]
fn output_shape_is_reported_alongside_the_sql() {
    let p = {
        let prog = parse_program(SRC).unwrap();
        let schema = build(&prog).unwrap();
        let reg = Registry::from_program(&prog);
        let q = bind(
            &schema,
            &reg,
            &parse_pipeline("products[name, price]").unwrap(),
            ordo_sema::Clock::CORPUS,
        )
        .unwrap();
        plan(&q, &test_space()).unwrap()
    };
    let names: Vec<&str> = p.outputs.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["name", "price"]);
    assert!(p.params.is_empty(), "nothing was left unbound");
}

#[test]
fn an_unprojected_sort_key_reaches_the_source_column() {
    // Ruling W047: the key is inferred from the source row and never added
    // to the output. Legal SQL for a plain select (not DISTINCT/GROUP —
    // which is why the fallback dies with row identity).
    let s = sql("products[name]\nsort price");
    assert!(s.contains(r#"ORDER BY "t0"."price""#), "{s}");
    // Check the SELECT clause only — a substring test over the whole
    // statement false-positives on `"price" ASC` in the ORDER BY.
    let select_clause = s.split_once("FROM").expect("has FROM").0;
    assert!(
        !select_clause.contains("price"),
        "the inferred key must not leak into the output:\n{s}"
    );
}

// ── 9f: the three aggregates the grammar was missing ─────────────────────

/// `percentile(field, p)` is the general form of `median`, which is its
/// p = 0.5 case — so it lowers to the same ordered-set aggregate.
#[test]
fn percentile_lowers_to_percentile_cont() {
    let s = sql("products[price, vendor]\ngroup vendor { p95: percentile(price, 0.95) }");
    assert!(
        s.contains("percentile_cont(0.95) WITHIN GROUP (ORDER BY"),
        "{s}"
    );
    // The same shape `median` already emits, with the fraction moved.
    let m = sql("products[price, vendor]\ngroup vendor { m: median price }");
    assert_eq!(s.replace("0.95", "0.5").replace("p95", "m"), m);
}

/// `distinct <field>` is count-distinct. It is a different construct from the
/// bare `distinct` STEP, which dedupes rows (W039's counting spelling) — that
/// one is a SELECT DISTINCT and is untouched.
#[test]
fn distinct_field_lowers_to_count_distinct() {
    let s = sql("products[vendor, name]\ngroup vendor { names: distinct name }");
    assert!(s.contains("count(DISTINCT "), "{s}");
    assert!(
        !s.contains("SELECT DISTINCT"),
        "the STEP was not meant: {s}"
    );

    let step = sql("products[name]\ndistinct");
    assert!(step.contains("SELECT DISTINCT"), "{step}");
    assert!(!step.contains("count(DISTINCT"), "{step}");
}

/// `joined(field, sep)` is ordered string aggregation: the ORDER BY lives
/// INSIDE the aggregate, because otherwise the concatenation order is
/// whatever the executor happened to produce, and the value is then not
/// reproducible.
#[test]
fn joined_lowers_to_an_ordered_string_agg() {
    let inner = |s: &str| {
        let agg = s
            .split("string_agg(")
            .nth(1)
            .expect("a string_agg is emitted")
            .to_string();
        agg[..agg.find(')').expect("closes")].to_string()
    };

    let s = sql("products[vendor, name]\ngroup vendor { names: joined(name, \", \") }");
    let i = inner(&s);
    assert!(i.contains("', '"), "the separator is a literal: {i}");
    assert!(
        i.contains("ORDER BY"),
        "an unordered string_agg is not reproducible: {i}"
    );

    // A declared sort before the group is the order the concatenation uses
    // (DL: "order = the pipeline's current sort, id tiebreak").
    let sorted =
        sql("products[vendor, name]\nsort -name\ngroup vendor { names: joined(name, \", \") }");
    assert!(
        inner(&sorted).contains("DESC"),
        "the pipeline's sort is ignored: {}",
        inner(&sorted)
    );
}

// ── the cost gate's analysis half ────────────────────────────────────────

#[test]
fn cost_classes_and_row_bounds() {
    use ordo_plan::cost::{classify, row_bound, CostClass, RowBound};
    let p = parse_program(SRC).unwrap();
    let schema = build(&p).unwrap();
    let registry = Registry::from_program(&p);
    let q = |src: &str| {
        bind(
            &schema,
            &registry,
            &parse_pipeline(src).unwrap(),
            ordo_sema::Clock::CORPUS,
        )
        .unwrap()
    };

    // Plain SQL, unbounded but pageable.
    let a = q("products[name]");
    assert_eq!(classify(&a), CostClass::Sql);
    assert_eq!(row_bound(&a, &schema), RowBound::Unbounded);

    // take n bounds at n; the bound survives later steps' wrap.
    let b = q("products[name, price]\nsort -price\ntake 2");
    assert_eq!(row_bound(&b, &schema), RowBound::AtMost(2));
    let b2 = q("top2[name]\nsort name");
    assert_eq!(
        row_bound(&b2, &schema),
        RowBound::AtMost(2),
        "callee bound survives the wrap"
    );

    // reduce provably yields one row.
    let c = q("products[price]\nreduce { m: max price }");
    assert_eq!(row_bound(&c, &schema), RowBound::AtMost(1));

    // Identity lookup: id pinned to a constant.
    let d = q("products(id == \"prod_1\")[name]");
    assert_eq!(row_bound(&d, &schema), RowBound::AtMost(1));

    // take[keys] is deliberately NOT evidence (ledger item).
    let e = q("products[name, vendor]\ntake[vendor] 1");
    assert_eq!(row_bound(&e, &schema), RowBound::Unbounded);
}

#[test]
fn audit_repros_stay_fixed() {
    use ordo_plan::cost::{classify, row_bound, CostClass, RowBound};
    let p = parse_program(SRC).unwrap();
    let schema = build(&p).unwrap();
    let registry = Registry::from_program(&p);
    let q = |src: &str| {
        bind(
            &schema,
            &registry,
            &parse_pipeline(src).unwrap(),
            ordo_sema::Clock::CORPUS,
        )
        .unwrap()
    };

    // Audit F1: a pinned anchor multiplied by a fanning source join is NOT
    // bounded — one vendor, many products.
    let a = q("vendors(id == \"v1\")[name]  products[price]");
    assert_eq!(row_bound(&a, &schema), RowBound::Unbounded);

    // Audit F2 (rename attack): an output NAMED id on a Sub anchor proves
    // nothing.
    let b = q("top2[id: name, price]\n| id == \"x\"");
    assert_eq!(
        row_bound(&b, &schema),
        RowBound::AtMost(2),
        "the callee's take bounds it; the fake id pin must not claim 1"
    );

    // Audit F2 (union attack): a genuine id filtered AFTER an append is
    // not identity evidence.
    let c = q("products[id, price]\nappend products[id, price]\n| id == \"p1\"");
    assert_eq!(row_bound(&c, &schema), RowBound::Unbounded);

    let _ = classify(&q("products[name]"));
    let _ = CostClass::Sql;
}

// ── 9i / G11: the pinned text collation ──────────────────────────────────

/// Every text ordering carries the pin, and nothing that is not text does.
///
/// The negative half matters as much as the positive: `COLLATE` on a
/// `bigint` or a `date` is a hard Postgres error, and on `bytea` — every
/// sealed column — it is a hard error too. A blanket collate would not
/// fail a "does it contain COLLATE" test; it would fail at the database.
#[test]
fn text_orderings_carry_the_pinned_collation() {
    // A declared text sort key, and the implicit id tiebreak behind it.
    let s = sql("products[name, price]\nsort name");
    let order = outer_order(&s);
    assert!(order.contains(r#""name" COLLATE "C" ASC"#), "{order}");
    assert!(
        order.contains(r#""id" COLLATE "C" ASC"#),
        "the id tiebreak is a TEXT ordering too: {order}"
    );

    // A ref column is text as well — its values are ids.
    let s = sql("products[name, vendor]\nsort vendor");
    assert!(
        outer_order(&s).contains(r#""vendor" COLLATE "C""#),
        "{}",
        outer_order(&s)
    );

    // Non-text keys take nothing.
    for (src, key) in [
        ("products[name, price]\nsort price", "price"),
        ("products[name, qty]\nsort qty", "qty"),
        ("products[name, due]\nsort due", "due"),
        ("products[name, active]\nsort active", "active"),
    ] {
        let s = sql(src);
        let order = outer_order(&s);
        assert!(order.contains(key), "{order}");
        assert!(
            !order.contains(&format!(r#""{key}" COLLATE"#)),
            "`{key}` has no collation to take: {order}"
        );
    }
}

/// `SELECT DISTINCT` needs its ORDER BY expressions to appear in the
/// select list verbatim, so the projection is spelled the collated way
/// too. Without this the plan is a hard Postgres error rather than a
/// wrong answer — but only for the shapes some test happens to cover,
/// which is why it is asserted rather than discovered.
#[test]
fn distinct_projects_the_same_spelling_it_orders_by() {
    let s = sql("products[name]\ndistinct");
    assert!(s.contains("SELECT DISTINCT"), "{s}");
    assert!(s.contains(r#""t0"."name" COLLATE "C" AS "name""#), "{s}");
    assert!(
        outer_order(&s).contains(r#""t0"."name" COLLATE "C""#),
        "{s}"
    );
}

/// `min`/`max` over text ARE orderings: they return the first or last
/// value in collation order. Every other aggregate in the closed set is
/// numeric or count-like and must stay uncollated.
#[test]
fn text_min_and_max_are_orderings_and_take_the_pin() {
    let s = sql("products[name, vendor]\ngroup vendor { first: min name, last: max name }");
    assert!(s.contains(r#"min("t0"."name" COLLATE "C")"#), "{s}");
    assert!(s.contains(r#"max("t0"."name" COLLATE "C")"#), "{s}");

    let s = sql("products[price, vendor]\ngroup vendor { lo: min price, hi: max price }");
    assert!(s.contains(r#"min("t0"."price")"#), "{s}");
    assert!(
        !s.contains(r#""price" COLLATE"#),
        "money has no collation: {s}"
    );
}

/// Ordered string aggregation concatenates in the pipeline's order, so a
/// deployment-dependent collation would change the STRING itself, not
/// merely the row sequence. Here the pipeline's order is the anchor id —
/// a text column, and therefore a text ordering.
#[test]
fn joined_orders_under_the_pin() {
    let s = sql("products[name, vendor]\ngroup vendor { names: joined(name, \", \") }");
    assert!(
        s.contains(
            r#"string_agg("t0"."name", ', ' ORDER BY "t0"."id" COLLATE "C" ASC NULLS LAST)"#
        ),
        "{s}"
    );
    // And when the pipeline declares a text order, that key takes it too.
    let s = sql("products[name, vendor]\nsort name\ngroup vendor { names: joined(name, \", \") }");
    assert!(
        s.contains(r#"ORDER BY "t0"."name" COLLATE "C" ASC NULLS LAST, "#),
        "{s}"
    );
}

/// The keyset seek predicate and the ORDER BY must be ONE ordering. The
/// same rendered key is used for both, which is the property that makes a
/// page neither skip nor repeat: the comparison the cursor is resolved by
/// is the comparison the rows were sorted by.
#[test]
fn the_keyset_seek_and_its_order_by_are_one_ordering() {
    use ordo_plan::plan_paged;
    let p = parse_program(SRC).unwrap();
    let schema = build(&p).unwrap();
    let registry = Registry::from_program(&p);
    let q = bind(
        &schema,
        &registry,
        &parse_pipeline("products[name, price]\nsort name").unwrap(),
        ordo_sema::Clock::CORPUS,
    )
    .unwrap();

    let paged = plan_paged(
        &q,
        &test_space(),
        2,
        Some(&[Some("ab".into()), Some("prod_1".into())]),
    )
    .unwrap();
    // `COLLATE` binds tighter than any comparison operator, so the seek's
    // `>` and its `IS NULL` both see the collated value.
    assert!(
        paged.sql.contains(r#""name" COLLATE "C" > 'ab'"#),
        "the seek compares under the pin:\n{}",
        paged.sql
    );
    assert!(
        paged.sql.contains(r#""$k1" COLLATE "C" > 'prod_1'"#),
        "including the hidden tiebreak key:\n{}",
        paged.sql
    );
    assert!(
        paged
            .sql
            .contains(r#"ORDER BY "name" COLLATE "C" ASC NULLS LAST"#),
        "and the ORDER BY under the same one:\n{}",
        paged.sql
    );
    // A key already projected is addressed by its output name rather than
    // duplicated into a hidden column — the match is on the bound
    // expression, which the COLLATE must not have broken.
    assert_eq!(
        paged.key_count, 1,
        "only the id tiebreak rides hidden:\n{}",
        paged.sql
    );
    assert_eq!(
        ordo_plan::order_key_outputs(&q),
        vec![Some("name".to_string()), None]
    );
}
