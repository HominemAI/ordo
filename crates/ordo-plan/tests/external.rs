//! Planning a read over an external entity (ruled 2026-11-15,
//! `external-entities`).
//!
//! An external reads as a subquery over the foreign table, never as a base
//! table and never as a view. It selects the raw key as `$key`, the typed id
//! as the prefix plus `_` plus the key as text, and each mapped column under
//! its field name. Every equality, join and sort on `id` uses `$key`, so the
//! foreign index serves. It carries no `$space` predicate: it is `shared`.

use ordo_plan::{plan, plan_paged, Space};
use ordo_schema::build;
use ordo_sema::{bind, Registry};
use ordo_syntax::{parse_pipeline, parse_program};

const SRC: &str = r#"
domain shop
external customers from crm.customers shared {
  id <- "customer_id"
  name: text <- "full_name"
  email: text? <- email
  account -> accounts? <- "account_id"
}
external accounts (key: acct) from crm.accounts shared {
  id: uuid <- ref
  label: text <- label
}
external regions (key: reg) from crm.regions shared {
  id: text <- code
  label: text <- label
  access read label == $caller
}
entity orders {
  customer -> customers
  total: int
}
op ordersWithNames = orders[total, customer.name]
op customerById($c: id) = customers(id == $c)[id, name]
op rename($id: id, $n: text) = set customers { id: $id, name: $n }
op drop($id: id) = del customers { id: $id }
"#;

fn setup() -> (ordo_schema::Schema, Registry) {
    let program = parse_program(SRC).expect("parses");
    let schema = build(&program).expect("schema");
    let mut registry = Registry::from_program(&program);
    registry.extend_from(&ordo_schema::std_ops(&schema));
    (schema, registry)
}

fn sql_of(pipeline: &str) -> String {
    let (schema, registry) = setup();
    let pl = parse_pipeline(pipeline).expect("parses");
    let q = bind(&schema, &registry, &pl, ordo_sema::Clock::CORPUS).expect("binds");
    plan(&q, &Space::bound("s1")).expect("plans").sql
}

/// The subquery an external reads through, in full. A change to this text
/// is a change to how every op over an external reaches the foreign table.
#[test]
fn an_external_reads_through_one_subquery() {
    let sql = sql_of("customers[id, name, email, account]");
    assert_eq!(
        sql,
        "SELECT\n  \"t0\".\"id\" AS \"id\",\n  \"t0\".\"name\" AS \"name\",\n  \"t0\".\"email\" AS \"email\",\n  \"t0\".\"account\" AS \"account\"\n\
         FROM (SELECT \"$ext\".\"customer_id\" AS \"$key\", 'cust_' || \"$ext\".\"customer_id\"::text AS \"id\", \
         \"$ext\".\"full_name\" AS \"name\", \"$ext\".\"email\" AS \"email\", \
         'acct_' || \"$ext\".\"account_id\"::text AS \"account\" \
         FROM \"crm\".\"customers\" AS \"$ext\") AS \"t0\"\n\
         ORDER BY \"t0\".\"$key\" ASC NULLS LAST;"
    );
    assert!(!sql.contains("$space"), "an external is shared: {sql}");
    assert!(!sql.contains("CREATE VIEW"), "{sql}");
}

#[test]
fn equality_on_an_external_id_uses_the_raw_key() {
    let sql = sql_of("customers(id == \"cust_42\")[id, name]");
    assert!(
        sql.contains(
            "(\"t0\".\"$key\" = (CASE WHEN 'cust_42' ~ '^cust_-?[0-9]{1,19}$' \
             THEN CASE WHEN CAST(substr('cust_42', 6) AS numeric) \
             BETWEEN -9223372036854775808 AND 9223372036854775807 \
             THEN CAST(substr('cust_42', 6) AS bigint) END END))"
        ),
        "{sql}"
    );
    // A uuid key decodes to a uuid, a text key by prefix alone.
    let sql = sql_of("accounts(id == \"acct_x\")[id]");
    assert!(sql.contains("AS uuid) END)"), "{sql}");
    let sql = sql_of("regions(id == \"reg_eu\")[id]");
    assert!(
        sql.contains("(CASE WHEN left('reg_eu', 4) = 'reg_' THEN substr('reg_eu', 5) END)"),
        "{sql}"
    );
}

#[test]
fn the_rust_key_check_takes_the_whole_bigint_range_and_nothing_else() {
    let (schema, _) = setup();
    let reads = ordo_sema::external_reads(&schema).expect("externals");
    let c = &reads["customers"];
    for (id, key) in [
        ("cust_42", Some("42")),
        ("cust_1000000000000000000", Some("1000000000000000000")),
        ("cust_9223372036854775807", Some("9223372036854775807")),
        ("cust_-9223372036854775808", Some("-9223372036854775808")),
        ("cust_9223372036854775808", None),
        ("cust_x", None),
        ("cust_", None),
        ("acct_42", None),
    ] {
        assert_eq!(ordo_plan::external_key_of(id, c), key, "{id}");
    }
    let a = &reads["accounts"];
    assert!(ordo_plan::external_key_of("acct_0b7e5f2a-1c3d-4e5f-8a9b-0c1d2e3f4a5b", a).is_some());
    assert!(ordo_plan::external_key_of("acct_0B7E5F2A-1C3D-4E5F-8A9B-0C1D2E3F4A5B", a).is_none());
}

#[test]
fn a_native_ref_joins_on_the_raw_key_and_keeps_the_row_when_it_is_gone() {
    let sql = sql_of("orders[total, customer.name]");
    // LEFT JOIN, so an order whose customer row is gone keeps its row and
    // reads the name as null. No space predicate on the external side; the
    // native side keeps its own.
    assert!(
        sql.contains(
            "LEFT JOIN (SELECT \"$ext\".\"customer_id\" AS \"$key\", 'cust_' || \"$ext\".\"customer_id\"::text AS \"id\""
        ),
        "{sql}"
    );
    assert!(
        sql.contains(
            ") AS \"t1\" ON \"t1\".\"$key\" = (CASE WHEN \"t0\".\"customer\" ~ '^cust_-?[0-9]{1,19}$' \
             THEN CASE WHEN CAST(substr(\"t0\".\"customer\", 6) AS numeric) \
             BETWEEN -9223372036854775808 AND 9223372036854775807 \
             THEN CAST(substr(\"t0\".\"customer\", 6) AS bigint) END END)\n"
        ),
        "{sql}"
    );
    assert!(sql.contains("WHERE \"t0\".\"$space\" = 's1'"), "{sql}");
    assert!(!sql.contains("\"t1\".\"$space\""), "{sql}");
}

#[test]
fn paging_sorts_and_seeks_by_the_raw_key() {
    let (schema, registry) = setup();
    let pl = parse_pipeline("customers[id, name] sort -id").expect("parses");
    let q = bind(&schema, &registry, &pl, ordo_sema::Clock::CORPUS).expect("binds");
    let page = plan_paged(&q, &Space::bound("s1"), 2, Some(&[Some("42".into())])).expect("plans");
    // The key rides as a hidden column and the cursor holds the raw key, so
    // `cust_10` follows `cust_9` and the foreign index serves the order.
    assert_eq!(page.key_count, 1, "{}", page.sql);
    assert!(
        page.sql.contains("\"t0\".\"$key\" AS \"$k0\""),
        "{}",
        page.sql
    );
    assert!(
        page.sql.contains("ORDER BY \"$k0\" DESC NULLS LAST"),
        "{}",
        page.sql
    );
    assert!(
        page.sql.contains("(\"$k0\" < '42' OR \"$k0\" IS NULL)"),
        "{}",
        page.sql
    );
}

#[test]
fn a_read_guard_over_the_externals_own_fields_applies() {
    let (schema, registry) = setup();
    let pl = parse_pipeline("regions[id, label]").expect("parses");
    let q = ordo_sema::bind_as(
        &schema,
        &registry,
        &pl,
        &[],
        Some("mem_1"),
        ordo_sema::Clock::CORPUS,
    )
    .expect("binds");
    let sql = plan(&q, &Space::bound("s1")).expect("plans").sql;
    assert!(sql.contains("\"t0\".\"label\""), "{sql}");
    assert!(sql.contains("'mem_1'"), "the guard reached SQL: {sql}");
}

#[test]
fn writes_to_an_external_are_refused_at_registration() {
    let (schema, registry) = setup();
    let e = ordo_plan::writes::check_writes(&schema, &registry).expect_err("refused");
    assert_eq!(e.code, "external_read_only");
    assert!(e.message.contains("read-only"), "{}", e.message);
}

#[test]
fn a_read_reaching_an_external_says_so() {
    let (schema, registry) = setup();
    let q = ordo_sema::bind_op_generic(
        &schema,
        &registry,
        "ordersWithNames",
        ordo_sema::Clock::CORPUS,
    )
    .expect("binds");
    assert_eq!(ordo_plan::externals_read(&q), ["customers"]);
    let pl = parse_pipeline("orders[total]").expect("parses");
    let q = bind(&schema, &registry, &pl, ordo_sema::Clock::CORPUS).expect("binds");
    assert!(ordo_plan::externals_read(&q).is_empty());
}

#[test]
fn the_descriptor_marks_an_external_and_lists_its_ops() {
    let (schema, _) = setup();
    let ents = ordo_plan::artifact::entity_artifacts(&schema);
    let c = ents
        .iter()
        .find(|e| e["entity"] == "customers")
        .expect("listed");
    assert_eq!(c["external"], true);
    assert_eq!(c["source"]["table"], "customers");
    let o = ents
        .iter()
        .find(|e| e["entity"] == "orders")
        .expect("listed");
    assert!(
        o.get("external").is_none(),
        "a native entity keeps its bytes"
    );
}

/// Review finding 5: an `exists` over an external nested inside arithmetic
/// (`cond(exists(customers …) -> 1, 0) + 1`) is still seen. It makes the
/// op read an external, and its `id` compares by the raw key inside the
/// subquery it renders.
#[test]
fn an_exists_nested_in_arithmetic_is_seen() {
    use ordo_sema::ir::{BCond, BExpr, From, OutField};
    use ordo_syntax::decl::ScalarTy;
    use ordo_syntax::{BinOp, CmpOp, Expr, Span};
    let (schema, registry) = setup();
    let pl = parse_pipeline("orders[total]").expect("parses");
    let mut q = bind(&schema, &registry, &pl, ordo_sema::Clock::CORPUS).expect("binds");
    assert!(ordo_plan::externals_read(&q).is_empty());

    let sp = Span::new(0, 0, 1, 1);
    let int = |n: i128| BExpr::Lit {
        value: Expr::IntLit { value: n, span: sp },
        ty: ScalarTy::Int,
    };
    let mut inner = q.clone();
    inner.from = From::Table {
        entity: "customers".into(),
        alias: "x9".into(),
    };
    inner.filters = vec![BCond::Cmp {
        op: CmpOp::Eq,
        lhs: BExpr::Col {
            alias: "x9".into(),
            column: "id".into(),
            ty: ScalarTy::Id,
        },
        rhs: BExpr::Col {
            alias: "t0".into(),
            column: "customer".into(),
            ty: ScalarTy::Id,
        },
    }];
    inner.outputs = Vec::new();
    inner.sort = Vec::new();
    inner.tiebreak = Vec::new();
    inner.externals = None;
    let exists = BCond::Exists {
        query: Box::new(inner),
        negated: false,
    };
    q.outputs.push(OutField {
        name: "flag".into(),
        expr: BExpr::Binary {
            op: BinOp::Add,
            lhs: Box::new(BExpr::Cond {
                arms: vec![(exists, int(1))],
                else_: Box::new(int(0)),
                ty: ScalarTy::Int,
            }),
            rhs: Box::new(int(1)),
            ty: ScalarTy::Int,
        },
    });
    assert_eq!(ordo_plan::externals_read(&q), ["customers"]);
    let sql = plan(&q, &Space::bound("s1")).expect("plans").sql;
    assert!(
        sql.contains("FROM \"crm\".\"customers\" AS \"$ext\") AS \"x9\""),
        "{sql}"
    );
    assert!(
        sql.contains("\"x9\".\"$key\" = (CASE WHEN \"t0\".\"customer\""),
        "{sql}"
    );
}

// ── `public` (ruled 2026-11-15, `external-entities` ask 5) ──────────────

const PUBLIC_SRC: &str = r#"
domain shop
external plans from crm.plans shared public {
  id <- "plan_id"
  name: text <- name
  price: int <- price
}
external accounts from crm.accounts shared {
  id <- id
  label: text public <- label
  secret: text <- secret
}
external hidden from crm.hidden shared {
  id <- id
  note: text <- note
}
"#;

fn public_setup() -> (ordo_schema::Schema, Registry) {
    let program = parse_program(PUBLIC_SRC).expect("parses");
    let schema = build(&program).expect("schema");
    let mut registry = Registry::from_program(&program);
    registry.extend_from(&ordo_schema::std_ops(&schema));
    (schema, registry)
}

#[test]
fn the_derived_reads_of_a_public_external_are_on_the_public_interface() {
    let (schema, registry) = public_setup();
    ordo_plan::cost::check_registration(&schema, &registry).expect("the public gate passes");
    for verb in ["get", "list", "count"] {
        let art = ordo_plan::artifact::op_artifact(&schema, &registry, &format!("std.plans.{verb}"))
            .expect("artifact");
        assert_eq!(art["flags"]["public"], true, "std.plans.{verb}");
        let art = ordo_plan::artifact::op_artifact(&schema, &registry, &format!("std.hidden.{verb}"))
            .expect("artifact");
        assert!(
            art["flags"].get("public").is_none(),
            "std.hidden.{verb} stays private"
        );
    }
}

#[test]
fn a_public_externals_derived_reads_scope_only_public_fields() {
    let (schema, registry) = public_setup();
    let read = |op: &str| {
        let scopes = ordo_plan::artifact::op_scope(&schema, &registry, op);
        assert_eq!(scopes.len(), 1, "{op}");
        scopes[0].read.iter().cloned().collect::<Vec<String>>()
    };
    assert_eq!(read("std.plans.get"), ["name", "price"]);
    assert_eq!(read("std.accounts.get"), ["label"]);
    assert_eq!(read("std.accounts.list"), ["label"]);
    assert_eq!(read("std.hidden.get"), ["note"]);
}

#[test]
fn an_authored_public_op_over_a_private_external_field_is_refused() {
    let program = parse_program(&format!(
        "{PUBLIC_SRC}\nop leak public = accounts[label, secret]\n  take 5\n"
    ))
    .expect("parses");
    let schema = build(&program).expect("schema");
    let registry = Registry::from_program(&program);
    let e = ordo_plan::cost::check_registration(&schema, &registry).expect_err("refused");
    assert_eq!(e.code, "public_reads_private");
}
