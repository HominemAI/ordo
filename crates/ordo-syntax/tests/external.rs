//! `external <name> from <schema>.<table> shared { … }` (ruled 2026-11-15,
//! `external-entities`): the declaration, and every refusal the parser can
//! decide on its own.

use ordo_syntax::decl::{ExternalKeyTy, FieldKind, ScalarTy};
use ordo_syntax::{
    parse_program, EXTERNAL_ACL, EXTERNAL_COMPOSITE_KEY, EXTERNAL_NOT_SHARED, EXTERNAL_OWNED,
    EXTERNAL_READ_ONLY, EXTERNAL_SEALED, EXTERNAL_UNSUPPORTED, PARSE_ERROR,
};

const CUSTOMERS: &str = r#"
external customers from crm.customers shared {
  id <- "customer_id"
  name: text <- "full_name"
  email: text? <- email
  account -> accounts <- "account_id"
  access read open
}
external accounts (key: acct) from crm.accounts shared {
  id: uuid <- "account_ref"
  label: text <- label
}
entity orders {
  customer -> customers
  total: int
}
"#;

#[test]
fn an_external_declares_its_table_key_and_columns() {
    let p = parse_program(CUSTOMERS).expect("parses");
    assert_eq!(p.externals.len(), 2);
    assert_eq!(p.entities.len(), 1, "an external is never an entity");

    let c = &p.externals[0];
    assert_eq!(c.name.name, "customers");
    assert_eq!(
        (c.schema.name.as_str(), c.table.name.as_str()),
        ("crm", "customers")
    );
    assert!(c.shared);
    assert_eq!(c.key_column.name, "customer_id");
    assert_eq!(
        c.key_ty,
        ExternalKeyTy::Int,
        "a bare `id <-` is an integer key"
    );
    assert!(c.key.is_none());
    assert!(c.access_read.is_some());
    let fields: Vec<(&str, &str, bool)> = c
        .fields
        .iter()
        .map(|f| (f.name.name.as_str(), f.column.name.as_str(), f.optional))
        .collect();
    assert_eq!(
        fields,
        [
            ("name", "full_name", false),
            ("email", "email", true),
            ("account", "account_id", false),
        ]
    );
    assert_eq!(c.fields[0].kind, FieldKind::Scalar(ScalarTy::Text));
    assert!(
        matches!(&c.fields[2].kind, FieldKind::Ref { target, owned: false, tree: false } if target.name == "accounts")
    );

    let a = &p.externals[1];
    assert_eq!(a.key.as_ref().map(|k| k.name.as_str()), Some("acct"));
    assert_eq!(a.key_ty, ExternalKeyTy::Uuid);
}

#[test]
fn the_schema_and_table_may_be_reserved_words() {
    // `public` is a keyword; a foreign table may still live there.
    let p = parse_program(
        "external prices from public.\"price_list\" shared {\n  id: text <- sku\n  cents: int <- cents\n}\n",
    )
    .expect("parses");
    assert_eq!(p.externals[0].schema.name, "public");
    assert_eq!(p.externals[0].key_ty, ExternalKeyTy::Text);
}

/// The class and a phrase the message must carry, so each refusal names
/// its fix and not only its rule.
fn refused(src: &str) -> (&'static str, String) {
    let e = parse_program(src).expect_err("refused");
    (e.code, e.message)
}

#[test]
fn shared_is_required_and_the_refusal_names_the_word() {
    let (code, msg) =
        refused("external customers from crm.customers {\n  id <- \"customer_id\"\n}\n");
    assert_eq!(code, EXTERNAL_NOT_SHARED);
    assert!(msg.contains("`shared`"), "{msg}");
    assert!(msg.contains("from crm.customers shared"), "{msg}");
}

#[test]
fn a_composite_key_is_refused_naming_one_column() {
    let (code, msg) =
        refused("external lines from crm.lines shared {\n  id <- \"order_id\", \"line_no\"\n}\n");
    assert_eq!(code, EXTERNAL_COMPOSITE_KEY);
    assert!(msg.contains("`id <- \"order_id\"`"), "{msg}");
}

#[test]
fn every_write_shaped_or_owner_shaped_clause_is_refused_by_name() {
    let head = "external customers from crm.customers shared {\n  id <- \"customer_id\"\n";
    for (body, class, says) in [
        ("  access write open\n", EXTERNAL_READ_ONLY, "read-only"),
        ("  acl (read, write)\n", EXTERNAL_ACL, "access read"),
        (
            "  name: text sealed <- \"full_name\"\n",
            EXTERNAL_SEALED,
            "Drop the word",
        ),
        (
            "  account -> accounts owned <- \"account_id\"\n",
            EXTERNAL_OWNED,
            "Drop `owned`",
        ),
        ("  unique (a, b)\n", EXTERNAL_UNSUPPORTED, "`unique`"),
        ("  index (a, b)\n", EXTERNAL_UNSUPPORTED, "`index`"),
        ("  check a > 0\n", EXTERNAL_UNSUPPORTED, "`check`"),
        ("  std sealed\n", EXTERNAL_UNSUPPORTED, "`std sealed`"),
        ("  public\n", EXTERNAL_UNSUPPORTED, "`public`"),
        (
            "  name: text unique <- \"full_name\"\n",
            EXTERNAL_UNSUPPORTED,
            "`unique`",
        ),
        ("  n: int = 0 <- n\n", EXTERNAL_UNSUPPORTED, "default"),
        (
            "  file: blob <- file\n",
            EXTERNAL_UNSUPPORTED,
            "`text` or `uri`",
        ),
    ] {
        let (code, msg) = refused(&format!("{head}{body}}}\n"));
        assert_eq!(code, class, "{body}: {msg}");
        assert!(msg.contains(says), "{body}: {msg}");
    }
}

#[test]
fn the_shape_errors_say_what_to_write() {
    for (src, says) in [
        (
            "external customers shared {\n  id <- \"customer_id\"\n}\n",
            "expected `from <schema>.<table>`",
        ),
        (
            "external customers from customers shared {\n  id <- \"customer_id\"\n}\n",
            "with its schema",
        ),
        (
            "external customers from crm.customers shared {\n  name: text <- \"full_name\"\n}\n",
            "names no key",
        ),
        (
            "external customers from crm.customers shared {\n  id <- \"customer_id\"\n  name: text \"full_name\"\n}\n",
            "expected `<-`",
        ),
        (
            "external customers from crm.customers shared {\n  id: bigint <- \"customer_id\"\n}\n",
            "`int`, `text` or `uuid`",
        ),
    ] {
        let (code, msg) = refused(src);
        assert_eq!(code, PARSE_ERROR, "{src}");
        assert!(msg.contains(says), "{src}: {msg}");
    }
}

#[test]
fn the_arrow_is_one_spelling_with_no_space_inside() {
    let (_, msg) =
        refused("external customers from crm.customers shared {\n  id < - \"customer_id\"\n}\n");
    assert!(msg.contains("expected `<-`"), "{msg}");
}

#[test]
fn external_stays_usable_as_a_field_name() {
    let p = parse_program("entity links { external: bool  url: text }\n").expect("parses");
    assert_eq!(p.entities[0].fields[0].name.name, "external");
}

#[test]
fn a_bare_underscore_name_is_told_to_take_quotes() {
    let e =
        parse_program("external customers from crm.customers shared {\n  id <- customer_id\n}\n")
            .expect_err("the lexer refuses underscores");
    assert!(e.message.contains("`\"customer_id\"`"), "{}", e.message);
}

#[test]
fn public_goes_after_shared_and_marks_fields() {
    let p = parse_program(
        r#"
external customers from crm.customers shared public {
  id <- "customer_id"
  name: text public <- "full_name"
  email: text? <- email
}
external plain from crm.plain shared {
  id <- id
}
"#,
    )
    .expect("parses");
    let c = &p.externals[0];
    let mark = c.public.expect("the line is public");
    assert!(!mark.cascade, "an external's public is never a cascade");
    assert!(c.fields[0].public, "name carries its own mark");
    assert!(!c.fields[1].public);
    assert!(p.externals[1].public.is_none());
}

#[test]
fn public_star_is_refused_naming_public() {
    for src in [
        "external c from crm.c shared public* {\n  id <- id\n}\n",
        "external c from crm.c shared {\n  id <- id\n  public*\n}\n",
        "external c from crm.c shared {\n  id <- id\n  n: text public* <- n\n}\n",
    ] {
        let (code, msg) = refused(src);
        assert_eq!(code, EXTERNAL_UNSUPPORTED, "{src}: {msg}");
        assert!(msg.contains("`public*`"), "{msg}");
        assert!(msg.contains("Write `public`"), "{msg}");
    }
}

#[test]
fn a_public_line_inside_the_braces_says_where_it_goes() {
    let (code, msg) = refused("external c from crm.c shared {\n  id <- id\n  public\n}\n");
    assert_eq!(code, EXTERNAL_UNSUPPORTED);
    assert!(msg.contains("after `shared`"), "{msg}");
}

#[test]
fn prev_on_an_external_field_is_refused_by_name() {
    let (code, msg) =
        refused("external c from crm.c shared {\n  id <- id\n  n: text prev(old) <- n\n}\n");
    assert_eq!(code, EXTERNAL_UNSUPPORTED);
    assert!(msg.contains("`prev`"), "{msg}");
    assert!(msg.contains("re-imported on every apply"), "{msg}");
}
