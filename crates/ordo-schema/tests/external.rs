//! External entities in the schema (ruled 2026-11-15, `external-entities`).
//!
//! An external is its own kind: [`Schema::externals`], never
//! [`Schema::entities`]. Ops and guards find it by name; the DDL, the diff,
//! dev reset and erase walk `entities` and never see it. The tests that
//! matter most here are the ones that prove Ordo plans no `CREATE`, `ALTER`
//! or `DROP` for a table another system owns.

use ordo_schema::diff::{Change, LiveColumn, LiveShape};
use ordo_schema::{build, ddl_for, diff, std_ops, FieldTy, Schema};
use ordo_syntax::decl::ExternalKeyTy;
use ordo_syntax::parse_program;

const SHOP: &str = r#"
domain shop
external customers from crm.customers shared {
  id <- "customer_id"
  name: text <- "full_name"
  email: text? <- email
  account -> accounts? <- "account_id"
  access read open
}
external accounts (key: acct) from crm.accounts shared {
  id: uuid <- ref
  label: text <- label
}
entity orders {
  customer -> customers
  total: int
}
"#;

fn schema_of(src: &str) -> Schema {
    build(&parse_program(src).expect("parses")).expect("builds")
}

fn refused(src: &str) -> (&'static str, String) {
    let e = build(&parse_program(src).expect("parses")).expect_err("refused");
    (e.code, e.message)
}

#[test]
fn an_external_is_its_own_kind() {
    let s = schema_of(SHOP);
    let names = |es: &[ordo_schema::Entity]| es.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&s.externals), ["customers", "accounts"]);
    assert!(
        !s.entities.iter().any(|e| e.external.is_some()),
        "no external is ever in `entities`"
    );
    assert!(s.native("customers").is_none());
    assert!(s.is_external("customers"));

    // Ops and guards find it by name, like any entity.
    let c = s.entity("customers").expect("found by name");
    assert_eq!(c.key, "cust");
    let src = c.external.as_ref().expect("carries its source");
    assert_eq!(
        (src.schema.as_str(), src.table.as_str()),
        ("crm", "customers")
    );
    assert_eq!(src.key_column, "customer_id");
    assert_eq!(src.key_ty, ExternalKeyTy::Int);
    assert_eq!(src.column("name"), Some("full_name"));
    // `id` and the declared fields; no `rev`, because Ordo never writes it.
    let fields: Vec<&str> = c.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(fields, ["id", "name", "email", "account"]);
    assert!(c.guards.is_some(), "its own `access read`");

    // A native ref to it is an ordinary ref field.
    let o = s.entity("orders").expect("native");
    assert!(
        matches!(&o.field("customer").unwrap().ty, FieldTy::Ref { target, .. } if target == "customers")
    );
}

#[test]
fn an_external_derives_the_reads_and_nothing_that_writes() {
    let s = schema_of(SHOP);
    let ops: Vec<String> = std_ops(&s)
        .ops
        .iter()
        .map(|o| o.name.name.clone())
        .filter(|n| n.starts_with("std.customers.") || n.starts_with("std.accounts."))
        .collect();
    assert_eq!(
        ops,
        [
            "std.customers.get",
            "std.customers.list",
            "std.customers.count",
            "std.accounts.get",
            "std.accounts.list",
            "std.accounts.count"
        ]
    );
}

// ── no DDL, ever ─────────────────────────────────────────────────────────

#[test]
fn the_ddl_creates_nothing_for_an_external_and_no_foreign_key_onto_it() {
    let ddl = ddl_for(&schema_of(SHOP)).expect("ddl");
    assert!(ddl.contains("CREATE TABLE \"orders\""), "{ddl}");
    for absent in ["\"customers\"", "\"accounts\"", "crm", "orders$customer$fk"] {
        assert!(!ddl.contains(absent), "`{absent}` in:\n{ddl}");
    }
}

fn col(ty: &str) -> LiveColumn {
    LiveColumn {
        ty: ty.into(),
        nullable: false,
        has_default: false,
    }
}

/// A database where `public` happens to hold tables with the externals'
/// names: the worst case for a planner that matched by name.
fn live_with_same_named_tables() -> LiveShape {
    let mut live = LiveShape::default();
    for t in ["customers", "accounts"] {
        live.column(t, "customer_id", col("bigint"));
        live.column(t, "full_name", col("text"));
    }
    live
}

fn mentions(changes: &[Change], name: &str) -> Vec<String> {
    changes
        .iter()
        .filter(|c| c.entity() == name)
        .map(|c| format!("{c:?}"))
        .collect()
}

#[test]
fn applying_a_program_with_an_external_plans_no_create_alter_or_drop_for_it() {
    let s = schema_of(SHOP);
    // A fresh database: only the native table is created.
    let fresh = diff(&LiveShape::default(), None, &s);
    assert!(mentions(&fresh, "customers").is_empty(), "{fresh:?}");
    assert!(mentions(&fresh, "accounts").is_empty(), "{fresh:?}");
    assert!(
        fresh
            .iter()
            .any(|c| matches!(c, Change::AddEntity { entity } if entity == "orders")),
        "{fresh:?}"
    );

    // Same-named tables in `public`, re-applied over itself: nothing.
    let live = live_with_same_named_tables();
    let again = diff(&live, Some(&s), &s);
    assert!(mentions(&again, "customers").is_empty(), "{again:?}");
    assert!(mentions(&again, "accounts").is_empty(), "{again:?}");
    assert!(
        !again
            .iter()
            .any(|c| matches!(c, Change::AddForeignKey { field, .. } if field == "customer")),
        "a ref to an external takes no foreign key: {again:?}"
    );
}

#[test]
fn removing_an_external_plans_no_drop() {
    let with = schema_of(
        "external customers from crm.customers shared {\n  id <- \"customer_id\"\n  name: text <- \"full_name\"\n}\nentity notes { body: text }\n",
    );
    let without = schema_of("entity notes { body: text }\n");
    let mut live = live_with_same_named_tables();
    live.column("notes", "body", col("text"));
    let changes = diff(&live, Some(&with), &without);
    assert!(
        mentions(&changes, "customers").is_empty(),
        "removing an external must never drop the other system's table: {changes:?}"
    );
}

#[test]
fn a_native_entity_turned_external_is_left_alone() {
    // The same name moves from Ordo's own table to another system's. The
    // planner neither drops Ordo's table nor alters it to fit.
    let prior = schema_of("entity customers { name: text }\n");
    let now = schema_of(
        "external customers from crm.customers shared {\n  id <- \"customer_id\"\n  label: text <- label\n}\n",
    );
    let mut live = LiveShape::default();
    live.column("customers", "name", col("text"));
    let changes = diff(&live, Some(&prior), &now);
    assert!(mentions(&changes, "customers").is_empty(), "{changes:?}");
}

// ── refusals the graph decides ───────────────────────────────────────────

#[test]
fn a_ref_from_an_external_to_a_native_entity_is_refused_naming_the_fix() {
    let (code, msg) = refused(
        "entity notes { body: text }\nexternal customers from crm.customers shared {\n  id <- \"customer_id\"\n  note -> notes <- \"note_id\"\n}\n",
    );
    assert_eq!(code, "external_ref_to_native");
    assert!(msg.contains("`note: text <- \"note_id\"`"), "{msg}");
    // `members` is Ordo's too.
    let (code, _) = refused(
        "external customers from crm.customers shared {\n  id <- \"customer_id\"\n  owner -> members <- owner\n}\n",
    );
    assert_eq!(code, "external_ref_to_native");
}

#[test]
fn a_native_entity_cannot_be_owned_by_an_external() {
    let (code, msg) = refused(
        "external customers from crm.customers shared {\n  id <- \"customer_id\"\n}\nentity notes {\n  customer -> customers owned\n  body: text\n}\n",
    );
    assert_eq!(code, "external_owned");
    assert!(msg.contains("Drop `owned`"), "{msg}");
}

#[test]
fn names_and_keys_are_one_namespace_with_the_native_entities() {
    let (code, _) = refused(
        "entity customers { name: text }\nexternal customers from crm.customers shared {\n  id <- \"customer_id\"\n}\n",
    );
    assert_eq!(code, "duplicate_entity");
    let (code, msg) = refused(
        "entity custards { name: text }\nexternal customers from crm.customers shared {\n  id <- \"customer_id\"\n}\n",
    );
    assert_eq!(code, "id_key_collision", "{msg}");
    let (code, _) = refused(
        "external customers from crm.customers shared {\n  id <- \"customer_id\"\n  rev: int <- rev\n}\n",
    );
    assert_eq!(code, "reserved_field_name");
}

#[test]
fn a_routine_may_neither_write_an_external_nor_wait_on_one() {
    let base = "domain shop\nexternal customers from crm.customers shared {\n  id <- \"customer_id\"\n  name: text <- \"full_name\"\n}\nentity orders { status: text }\n";
    let (code, _) = refused(&format!(
        "{base}routine r {{\n  trigger every 5m\n  run:\n    set customers(name == \"a\") {{ name: \"b\" }}\n}}\n"
    ));
    assert_eq!(code, "external_read_only");
    let (code, msg) = refused(&format!(
        "{base}routine r {{\n  trigger when shop.customers.created\n  run:\n    set orders {{ status: \"b\" }}\n}}\n"
    ));
    assert_eq!(code, "external_live");
    assert!(msg.contains("trigger every"), "{msg}");
}

// ── `public` (ruled 2026-11-15, `external-entities` ask 5) ──────────────

const PUBLIC_SHOP: &str = r#"
domain shop
external customers from crm.customers shared public {
  id <- "customer_id"
  name: text <- "full_name"
  email: text? <- email
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

#[test]
fn the_public_mark_lands_on_an_external_and_its_fields() {
    let s = schema_of(PUBLIC_SHOP);
    let customers = &s.externals[0];
    let mark = customers.public.as_ref().expect("the line is public");
    assert!(!mark.cascade && mark.from.is_none());
    let public = |e: &ordo_schema::Entity| {
        e.declared()
            .filter(|f| f.public)
            .map(|f| f.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(public(customers), ["name", "email"], "the line reaches all");
    assert!(customers.has_public());
    assert!(
        !customers.field("id").expect("id").public,
        "the platform's id is never marked"
    );

    let accounts = &s.externals[1];
    assert!(accounts.public.is_none());
    assert_eq!(public(accounts), ["label"], "a field mark reaches one field");

    let hidden = &s.externals[2];
    assert!(!hidden.has_public());
}

#[test]
fn only_a_public_externals_derived_reads_are_public_and_carry_public_fields() {
    let s = schema_of(PUBLIC_SHOP);
    let ops = std_ops(&s).ops;
    let find = |n: &str| ops.iter().find(|o| o.name.name == n).expect(n);
    for verb in ["get", "list", "count"] {
        assert!(find(&format!("std.customers.{verb}")).flags.public);
        assert!(find(&format!("std.accounts.{verb}")).flags.public);
        assert!(!find(&format!("std.hidden.{verb}")).flags.public);
    }
    let projected = |n: &str| match &find(n).body {
        ordo_syntax::decl::OpBody::Read(pl) => pl.sources[0]
            .projection
            .as_ref()
            .expect("projects")
            .iter()
            .map(|p| match p {
                ordo_syntax::decl::ProjItem::Field { path, .. } => path[0].name.clone(),
                _ => unreachable!("only plain fields"),
            })
            .collect::<Vec<_>>(),
        _ => unreachable!("a read"),
    };
    assert_eq!(projected("std.accounts.get"), ["id", "label"]);
    assert_eq!(projected("std.hidden.get"), ["id", "note"]);
}

#[test]
fn a_guarded_public_external_is_refused_like_a_native_one() {
    let (code, _) = refused(
        r#"
external customers from crm.customers shared public {
  id <- id
  owner: text <- owner
  access read owner == $caller
}
"#,
    );
    assert_eq!(code, "public_guarded");
}
